//! [`Credentials`]: every operation that issues, rotates or revokes a login.
//!
//! The rule this file exists to hold is one paragraph of `docs/data-model.md`,
//! "Users and authentication", restated by ADR 0025:
//!
//! > Login, refresh, reset-link issuance, password changes and reset-token
//! > consumption lock the user row before locking or writing that user's token
//! > rows. Re-read and validate credentials under that lock. Expensive password
//! > hashing may happen beforehand, but an earlier password check must be
//! > revalidated against the locked row before issuing credentials. [...]
//! > Return credentials only after commit.
//!
//! So every method below has the same five-part shape: the cheap rejections,
//! then the slow unlocked work (an Argon2 hash is tens of milliseconds and a
//! held row lock has no business paying for them), then `BEGIN` and
//! [`UserRepository::lock_user`], then the decision *redone* against the row
//! the lock returned, then `COMMIT` — and the access token and the cookie are
//! built from the committed row, never before. That is what makes a concurrent
//! password change either revoke the new credential or be observed by it,
//! never neither.
//!
//! **Lock order.** One user-row lock, then that user's token rows.
//! [`Credentials::accept_invite`] is the one exception in the crate, and only
//! because the user it is about does not exist yet: it locks the *invite* row
//! and creates the user and its first refresh token under that lock. No
//! project, session or administrator-membership lock is taken here — accepting
//! an invite can only ever add an administrator (`SPEC.md`, "Users").
//!
//! **Throttling** is wired in here too rather than in the login route, because
//! "check before hashing, count every rejection, clear on success" is part of
//! the same order: a throttle a caller drives from outside is a throttle a
//! caller can drive wrongly.

use std::net::IpAddr;

use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::Cookie;
use chrono::Utc;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::cookies::{clear_refresh_cookie, presented_token, refresh_cookie};
use crate::auth::public_link;
use crate::email::EmailMessage;
use crate::models::user::{hash_password, verify_password};
use crate::models::{Email, NewUser, OpaqueToken, Password, User, UserInvite, Username};
use crate::prelude::*;
use crate::repositories::{
    PasswordResetTokenRepository, RefreshTokenRepository, UserInviteRepository, UserRepository,
    password_reset_tokens, user_invites,
};

/// What a failed refresh or logout-shaped authentication answers with, the
/// same string `routes::extractors` uses (`SPEC.md`, "Authentication").
const AUTHENTICATION_REQUIRED: &str = "authentication required";

/// What login answers for an unknown username *and* for a wrong password.
///
/// One message for both: which of the two a caller got wrong is exactly what
/// an account-enumeration probe is after.
const INVALID_CREDENTIALS: &str = "invalid username or password";

/// The 429 message (`SPEC.md`, "Auth (`/api/auth`)").
///
/// No `Retry-After` travels with it: the block's remaining time is a schedule
/// for the next round of guesses, and `SPEC.md` fixes the response as the
/// status and this string.
const TOO_MANY_LOGIN_ATTEMPTS: &str = "too many login attempts";

/// What `POST /auth/reset-password` answers for a token that is unknown,
/// already spent, expired, or issued before a password change that
/// invalidated it.
///
/// One message for all four, for the reason
/// [`PasswordResetTokenRepository::find_valid_by_hash_for_user`] gives:
/// telling a caller which of them applies is telling them something about
/// somebody else's account.
const INVALID_RESET_TOKEN: &str = "invalid or expired token";

/// What both invite routes answer for a token that is unknown, already
/// accepted or expired (`SPEC.md`, "Auth (`/api/auth`)": "400 if expired, used
/// or unknown").
///
/// One message for all three, and deliberately the same one from the lookup
/// and from the acceptance: an unauthenticated caller holding a guessed token
/// learns only that it is not a live invitation, never that an address was
/// invited and has already signed up.
const INVALID_INVITE: &str = "invalid or expired invite";

/// The 400 for a self-service change that left `current_password` out.
const CURRENT_PASSWORD_REQUIRED: &str = "current password required";

/// The 400 for a self-service change whose `current_password` did not verify
/// against the locked row.
const CURRENT_PASSWORD_INCORRECT: &str = "current password is incorrect";

/// The path segment of an emailed invitation link.
const INVITE_SEGMENT: &str = "invite";

/// The path segment of an emailed password-reset link.
const RESET_SEGMENT: &str = "reset-password";

/// An Argon2id hash a login for an unknown username is verified against.
///
/// Not a credential: the PHC string of the obviously fake password
/// `not-a-real-password`, generated once with
/// [`crate::models::user::hash_password`] and pasted here (`CLAUDE.md`, rule 3).
/// Nothing authenticates against it and no row carries it.
///
/// Its purpose is timing. Without it, a login for a name that exists costs one
/// Argon2 verification and a login for a name that does not costs none, and
/// the difference is measurable from outside — which would turn the
/// deliberately identical [`INVALID_CREDENTIALS`] message back into an
/// enumeration oracle. Verifying the submitted password against this hash
/// spends the same work and throws the answer away.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$8oilaKuGRDWMSAJB+d33OQ$mq7rf+s/87LSWeNRWB94b5LtVYCVoG9hLW46mzmZ5N0";

/// A freshly issued login: the user it belongs to, its access token and the
/// cookie carrying its refresh token.
///
/// Produced only by [`Credentials`], and only after the transaction that
/// created the refresh-token row has committed. A route adds
/// [`IssuedPair::refresh_cookie`] to its jar and serialises the other two; it
/// does not get to assemble one of these itself, which is what makes "return
/// credentials only after commit" structural rather than remembered.
#[derive(Debug)]
pub struct IssuedPair {
    /// The user as read under the lock and after the mutation, so its
    /// `auth_version`, `admin` and `must_change_password` are the committed
    /// ones.
    pub user: User,
    /// The signed JWT, minted from [`IssuedPair::user`].
    pub access_token: String,
    /// The `refresh_token` cookie, with the attributes `SPEC.md`,
    /// "Authentication" fixes.
    pub refresh_cookie: Cookie<'static>,
}

/// The crate's one issuer of access tokens, refresh cookies, invitation links
/// and reset links.
///
/// Borrows [`AppState`] for its lifetime, the way a repository borrows the
/// pool: a route builds one per request and drops it with the response.
pub struct Credentials<'a> {
    state: &'a AppState,
}

impl<'a> Credentials<'a> {
    /// Borrow `state` for the lifetime of this value.
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// Exchange a username and password for a pair.
    ///
    /// The order is the contract, not an implementation detail:
    ///
    /// 1. the throttle, *before* any hashing, so a blocked key costs one
    ///    hash-free request and never extends its own block (`SPEC.md`,
    ///    "Authentication");
    /// 2. the unlocked lookup and the Argon2 verification, off the runtime's
    ///    worker threads;
    /// 3. the user-row lock, the revalidation against the locked row and the
    ///    insert, in one transaction;
    /// 4. `COMMIT`, then the access token, the cookie and
    ///    [`crate::routes::throttle::LoginThrottle::record_success`].
    ///
    /// `username` is used as given — the caller trims it, because the throttle
    /// keys on the same string. `client` is the address the throttle's other
    /// key is (`crate::routes::throttle::client_addr`).
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        client: IpAddr,
    ) -> Result<IssuedPair> {
        if self.state.login_throttle.check(username, client).is_err() {
            // Deliberately no `record_failure`: a blocked request must not push
            // its own block further out (`SPEC.md`, "Authentication").
            info!(username = %username, addr = %client, "login refused: throttled");
            return Err(Error::Throttled(TOO_MANY_LOGIN_ATTEMPTS.to_string()));
        }

        let users = UserRepository::new(&self.state.pool);
        let candidate = users.find_by_username(username).await?;

        // One Argon2 verification either way; see `DUMMY_PASSWORD_HASH`.
        let hash = candidate
            .as_ref()
            .map_or(DUMMY_PASSWORD_HASH, |user| user.password_hash.as_str())
            .to_string();
        let verified = verify_blocking(hash, password.to_string()).await?;

        let (Some(user), true) = (candidate, verified) else {
            return Err(self.reject_login(username, client));
        };

        let mut tx = self.state.pool.begin().await?;

        // From here on the row this returns is the only authority: a password
        // change may have committed while this request waited for the lock.
        let Some(locked) = users.lock_user(&mut tx, user.id).await? else {
            // Deleted between the lookup and the lock.
            return Err(self.reject_login(username, client));
        };

        // Revalidate the password against the locked row (`docs/data-model.md`;
        // ADR 0025). An unchanged hash means the verification above was about
        // exactly this row and still stands; a changed one is re-verified rather
        // than assumed wrong, because a user may have "changed" their password to
        // the same string.
        if locked.password_hash != user.password_hash
            && !verify_blocking(locked.password_hash.clone(), password.to_string()).await?
        {
            return Err(self.reject_login(username, client));
        }

        let pair = self.issue(&mut tx, locked).await?;
        tx.commit().await?;

        self.state.login_throttle.record_success(username);

        Ok(pair)
    }

    /// Rotate the presented refresh token and mint a new pair from the current
    /// user row.
    ///
    /// Authenticated by the cookie alone, so every rejection is the same 401 as
    /// a missing cookie; the caller clears the cookie on any 401 it gets back
    /// ([`Credentials::clearing_cookie`]) and leaves it alone on a 5xx — a
    /// refresh that failed because Postgres was briefly unreachable is a retry,
    /// not a sign-out.
    ///
    /// The old token is revoked and the replacement inserted in the same
    /// transaction, under the user-row lock: the authoritative read of the
    /// presented token happens *after* the lock is granted, which is where a
    /// refresh racing a password change loses (ADR 0025).
    pub async fn refresh(&self, jar: &CookieJar) -> Result<IssuedPair> {
        let Some(raw) = presented_token(jar) else {
            return Err(unauthorized());
        };

        let hash = OpaqueToken::hash_of(&raw);
        let tokens = RefreshTokenRepository::new(&self.state.pool);
        let users = UserRepository::new(&self.state.pool);

        // Unlocked, and only to learn which user row to lock: a request arrives
        // with a cookie and nothing else. Nothing is decided from this row —
        // `revoked_at` and `expires_at` are read again under the lock below.
        let Some(located) = tokens.find_by_hash(&hash).await? else {
            return Err(unauthorized());
        };

        let mut tx = self.state.pool.begin().await?;

        let Some(user) = users.lock_user(&mut tx, located.user_id).await? else {
            // A cookie for a user who has been deleted.
            return Err(unauthorized());
        };

        // The authoritative read. A password change that committed while this
        // request waited for the lock has already set `revoked_at`, so this is
        // where the revocation race is decided (ADR 0025).
        let Some(current) = tokens
            .find_by_hash_for_user(&mut tx, &hash, user.id)
            .await?
        else {
            return Err(unauthorized());
        };

        if !current.is_usable(Utc::now()) {
            debug!(user_id = %user.id, "refresh token is revoked or expired");
            return Err(unauthorized());
        }

        tokens.revoke(&mut tx, current.id).await?;
        let pair = self.issue(&mut tx, user).await?;
        tx.commit().await?;

        Ok(pair)
    }

    /// Revoke the presented refresh token and return the cookie that clears it.
    ///
    /// Never fails over a cookie: logging out is not an operation that can be
    /// refused, and telling a caller that their cookie named nothing would be
    /// an oracle over stored tokens. Only the presented token is revoked — the
    /// user's other browsers stay signed in — so no user-row lock is needed.
    pub async fn logout(&self, jar: &CookieJar) -> Result<Cookie<'static>> {
        if let Some(raw) = presented_token(jar) {
            RefreshTokenRepository::new(&self.state.pool)
                .revoke_by_hash(&OpaqueToken::hash_of(&raw))
                .await?;
        }

        Ok(self.clearing_cookie())
    }

    /// Spend an invitation: create the user it names and sign them in.
    ///
    /// The only way a user who was not seeded by the first migration comes into
    /// existence (ADR 0013), so everything that makes a user is decided here,
    /// and the order is the contract:
    ///
    /// 1. the username and the password validated, and the password hashed, all
    ///    *before* `BEGIN` — a malformed body must never reach the invite lock;
    /// 2. `BEGIN` and [`UserInviteRepository::lock_open_by_hash`], which is both
    ///    the authoritative validity check and the serialisation point: two
    ///    browsers submitting the same link do not race, the second waits and
    ///    then finds no open invite;
    /// 3. the insert into `users`, whose two unique constraints decide the 409s
    ///    — a username somebody else took while this form was open, and an
    ///    address that has become a user by some other path;
    /// 4. [`UserInviteRepository::mark_accepted`] and the refresh token, then
    ///    `COMMIT`.
    ///
    /// All of steps 2 to 4 are one transaction, so a failure at any of them
    /// leaves neither a user without a spent invite nor a spent invite without a
    /// user. A 409 in step 3 in particular rolls the invite back to open, which
    /// is what lets the invitee simply try another username — and, when it was
    /// the *address* that clashed, lets an administrator revoke an invitation
    /// that can no longer be accepted.
    ///
    /// The token is trimmed: it is a JSON string a client assembled, and one
    /// pasted with a trailing newline is the same invitation.
    ///
    /// `must_change_password` is false: the invitee chose this password seconds
    /// ago, and the flag exists for the seeded administrator's documented
    /// default (`docs/data-model.md`, `users`). `notify_email` comes from the
    /// column default, which is true.
    pub async fn accept_invite(
        &self,
        token: &str,
        username: &str,
        password: &str,
    ) -> Result<IssuedPair> {
        // Cheap rejections first, then the hash, then the transaction.
        let username = Username::parse(username)?;
        let password = Password::parse(password)?;

        let token_hash = OpaqueToken::hash_of(token.trim());
        let password_hash = hash_blocking(password).await?;

        let invites = UserInviteRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;

        let Some(invite) = invites.lock_open_by_hash(&mut tx, &token_hash).await? else {
            return Err(invalid_invite());
        };

        let new_user = NewUser {
            id: Uuid::new_v4(),
            username,
            // The stored address, not one the caller sent. `Email::parse` is
            // idempotent on an already normalised value, so this re-parse is a
            // type conversion rather than a second normalisation.
            email: Email::parse(&invite.email)?,
            password_hash,
            admin: invite.admin,
            must_change_password: false,
        };

        let user = UserRepository::new(&self.state.pool)
            .insert(&mut tx, &new_user)
            .await?;

        // Under the row lock this can only be true; the guard is what makes the
        // acceptance correct without depending on the caller having locked.
        if !invites.mark_accepted(&mut tx, invite.id, user.id).await? {
            return Err(invalid_invite());
        }

        let (invite_id, user_id, admin) = (invite.id, user.id, user.admin);
        let pair = self.issue(&mut tx, user).await?;
        tx.commit().await?;

        // After the commit, and the two ids and the role only: never the
        // token and never the address.
        info!(invite_id = %invite_id, user_id = %user_id, admin, "invite accepted");

        Ok(pair)
    }

    /// Mail a reset link, or quietly do nothing.
    ///
    /// The caller answers 204 either way and this returns `Ok(())` for a known
    /// identifier, an unknown one, a rate-limited one and a failed delivery
    /// alike (`SPEC.md`, "Auth (`/api/auth`)": "→ 204 (always)"). Anything else
    /// would turn the endpoint into an account-enumeration oracle, which is the
    /// whole reason it is shaped like this.
    ///
    /// The limiter call and the lookup therefore both happen for *every*
    /// request, before either result is consulted: an early return on an
    /// unknown identifier would skip the limiter and make the two paths differ
    /// in work done, which is measurable even when the body is not. What the
    /// two paths do not spend is an Argon2 hash — neither of them hashes
    /// anything, so there is no asymmetry to hide.
    ///
    /// The insert is under the user-row lock, the way `docs/data-model.md`
    /// requires of reset-link issuance: a password change committing between
    /// the lock and this insert would otherwise leave a live link behind that
    /// its own invalidation had already passed by (ADR 0025). The mail goes out
    /// *after* the commit, so a link can never be delivered for a transaction
    /// that rolled back.
    pub async fn request_password_reset(&self, identifier: &str) -> Result<()> {
        let users = UserRepository::new(&self.state.pool);

        // Both, unconditionally, and in this order: `allow` records the request,
        // so a limited identifier costs a lookup too.
        let allowed = self.state.reset_rate_limit.allow(identifier);
        let candidate = users.find_by_username_or_email(identifier).await?;

        let (Some(user), true) = (candidate, allowed) else {
            // Never the identifier at `info`: it is a username or an email
            // address somebody typed, and this is the one endpoint an
            // unauthenticated stranger can write to.
            debug!(allowed, "password reset request not acted on");
            return Ok(());
        };

        let token = OpaqueToken::generate();
        let expires_at = password_reset_tokens::expires_at(Utc::now());

        let mut tx = self.state.pool.begin().await?;

        let Some(locked) = users.lock_user(&mut tx, user.id).await? else {
            // Deleted between the lookup and the lock. Nothing to mail.
            return Ok(());
        };

        PasswordResetTokenRepository::new(&self.state.pool)
            .insert(&mut tx, locked.id, &token.hash, expires_at)
            .await?;
        tx.commit().await?;

        let link = public_link(&self.state.config, RESET_SEGMENT, &token.raw);
        let message = EmailMessage::password_reset(&locked.email, &link, expires_at);

        // A provider failure is the operator's problem, not the caller's: the
        // row is committed, the answer stays 204, and the user can ask again.
        match self.state.email.send(message).await {
            Ok(()) => info!(user_id = %locked.id, "password reset link sent"),
            Err(err) => {
                error!(user_id = %locked.id, error = %err, "the password reset email failed");
            }
        }

        Ok(())
    }

    /// Spend a reset link and set a new password. Nobody is logged in.
    ///
    /// "Reset by link returns 204 without logging the user in; they then log in
    /// with the new password" (`SPEC.md`, "Authentication"), which is why
    /// [`UserRepository::apply_password_change`] is called with `None`: every
    /// one of the user's refresh tokens is revoked and none is issued.
    ///
    /// The sequence is the one `docs/data-model.md`, `password_reset_tokens`
    /// prescribes, and the order is the contract:
    ///
    /// 1. the unlocked [`PasswordResetTokenRepository::find_by_hash`], purely to
    ///    learn which user row to lock — a presented token is all there is to go
    ///    on;
    /// 2. the new password validated and hashed, *outside* any transaction,
    ///    because Argon2 is tens of milliseconds and a user-row lock is not a
    ///    place to spend them;
    /// 3. `BEGIN`, the user-row lock, and
    ///    [`PasswordResetTokenRepository::find_valid_by_hash_for_user`] under it
    ///    — the read the decision is actually made on, which is what makes a
    ///    token invalidated by a concurrent password change lose the race rather
    ///    than win it;
    /// 4. the mutation and `COMMIT`.
    ///
    /// Step 2 sits where it does rather than after step 3 because the hash has
    /// to exist before the transaction opens; a caller whose token is already
    /// invalid never reaches it, because step 1 has refused them.
    pub async fn reset_password(&self, token: &str, password: &str) -> Result<()> {
        let token_hash = OpaqueToken::hash_of(token);
        let tokens = PasswordResetTokenRepository::new(&self.state.pool);

        let Some(located) = tokens.find_by_hash(&token_hash).await? else {
            return Err(invalid_reset_token());
        };

        let password = Password::parse(password)?;
        let password_hash = hash_blocking(password).await?;

        let users = UserRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;

        let Some(locked) = users.lock_user(&mut tx, located.user_id).await? else {
            // The user was deleted; the token row went with them or is about to.
            return Err(invalid_reset_token());
        };

        if tokens
            .find_valid_by_hash_for_user(&mut tx, &token_hash, locked.id)
            .await?
            .is_none()
        {
            return Err(invalid_reset_token());
        }

        users
            .apply_password_change(&mut tx, locked.id, &password_hash, None)
            .await?;
        tx.commit().await?;

        info!(user_id = %locked.id, "password reset through an emailed link");

        Ok(())
    }

    /// Change `user_id`'s password, optionally keeping the acting browser
    /// signed in.
    ///
    /// Both flows `SPEC.md`, "Authentication" describes, told apart by
    /// `replace_for_browser` rather than by who is calling: the *authorization*
    /// — whose id this may be pointed at — belongs to the route, and everything
    /// after it belongs here.
    ///
    /// - `replace_for_browser` true is the self-service change. `current` is
    ///   required and verified against the row the lock returned, not against a
    ///   copy read earlier, and the result carries a pair. The replacement
    ///   refresh token is created *by*
    ///   [`UserRepository::apply_password_change`], after its blanket
    ///   revocation and inside the same transaction: a token inserted before the
    ///   revocation would revoke itself, and one inserted after the commit would
    ///   leave a window in which the browser holds no usable token at all. A
    ///   vanished user is 401 — it is the caller's own row.
    /// - `replace_for_browser` false is an administrator setting somebody
    ///   else's password, or a caller who does not want to stay signed in.
    ///   `current` is ignored rather than validated — the whole point is that
    ///   the administrator does not know it — every one of the target's tokens
    ///   is revoked, none is issued, and the result is `None`. A vanished user
    ///   is 404.
    ///
    /// The Argon2 verification in the self-service flow is spent while holding
    /// the row lock. That is the sanctioned cost: the alternative is verifying
    /// twice, and the lock is per user, so what waits behind it is that one
    /// user's own concurrent credential work.
    pub async fn change_password(
        &self,
        user_id: Uuid,
        current: Option<String>,
        new: &str,
        replace_for_browser: bool,
    ) -> Result<Option<IssuedPair>> {
        // Before the hash, so a self-service request that forgot the field
        // costs no Argon2 work.
        let current = if replace_for_browser {
            let Some(current) = current else {
                debug!(user_id = %user_id, "password change refused: no current password");
                return Err(Error::BadRequest(CURRENT_PASSWORD_REQUIRED.to_string()));
            };
            Some(current)
        } else {
            None
        };

        let password = Password::parse(new)?;
        let password_hash = hash_blocking(password).await?;

        let users = UserRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;

        let Some(locked) = users.lock_user(&mut tx, user_id).await? else {
            return Err(if replace_for_browser {
                // Deleted between the extractor's read and the lock; it is the
                // caller's own row, so this is a failed authentication.
                Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string())
            } else {
                Error::NotFound
            });
        };

        if let Some(current) = current
            && !verify_blocking(locked.password_hash.clone(), current).await?
        {
            debug!(user_id = %user_id, "password change refused: wrong current password");
            return Err(Error::BadRequest(CURRENT_PASSWORD_INCORRECT.to_string()));
        }

        // `Some` only for the self-service flow, and generated here so the raw
        // half never leaves this function except inside the cookie.
        let replacement = replace_for_browser.then(OpaqueToken::generate);
        let updated = users
            .apply_password_change(
                &mut tx,
                locked.id,
                &password_hash,
                replacement.as_ref().map(|token| token.hash.as_str()),
            )
            .await?;
        tx.commit().await?;

        info!(user_id = %updated.id, replaced = replacement.is_some(), "password changed");

        // From the committed row, so the claims carry the new `auth_version`
        // and the cleared `must_change_password` — which is what makes the
        // token minted a moment ago stop working and this one start.
        let Some(replacement) = replacement else {
            return Ok(None);
        };

        Ok(Some(IssuedPair {
            access_token: self.access_token(&updated)?,
            refresh_cookie: refresh_cookie(&self.state.config, &replacement.raw),
            user: updated,
        }))
    }

    /// Issue an invitation for `email` and mail its link.
    ///
    /// The raw token exists only in that link: the row stores its SHA-256, the
    /// returned [`UserInvite`] carries neither, and nothing here logs either
    /// (rule 3, ADR 0026). The sequence is the one `SPEC.md`, "Users" and
    /// `docs/data-model.md`, `user_invites` prescribe between them:
    ///
    /// 1. `BEGIN`,
    /// 2. [`UserInviteRepository::delete_expired_open_for_email`] — the partial
    ///    unique index is blind to `expires_at`, so a dead invite would
    ///    otherwise refuse the replacement,
    /// 3. [`UserInviteRepository::insert`], which decides *both* conflicts in
    ///    one statement: an address that already belongs to a user, and a second
    ///    open invite for the address,
    /// 4. `COMMIT`, and only then the email.
    ///
    /// Steps 2 and 3 share a transaction so the address is never left with no
    /// invite at all. The email goes out after the commit, so a mail failure
    /// leaves the invite row standing and
    /// [`Credentials::resend_invite`] recovers it; the alternative — sending
    /// inside the transaction — would deliver links to invites that rolled back.
    pub async fn create_invite(
        &self,
        email: &Email,
        admin: bool,
        invited_by: Uuid,
    ) -> Result<UserInvite> {
        let token = OpaqueToken::generate();
        let expires_at = user_invites::expires_at(Utc::now());

        let invites = UserInviteRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;
        invites
            .delete_expired_open_for_email(&mut tx, email)
            .await?;
        let invite = invites
            .insert(
                &mut tx,
                email,
                &token.hash,
                admin,
                Some(invited_by),
                expires_at,
            )
            .await?;
        tx.commit().await?;

        info!(invite_id = %invite.id, invited_by = %invited_by, admin, "invite created");

        self.deliver_invite(&invite, &token.raw).await?;

        Ok(invite)
    }

    /// Mint a new token for an unaccepted invite and mail it again.
    ///
    /// A *new* token rather than the old one, so one invite always has exactly
    /// one live link and the earlier email stops working the moment this commits
    /// (`SPEC.md`, "Users"). The expiry restarts at [`INVITE_TTL`], which is
    /// what makes this the recovery path both for an invite that lapsed and for
    /// one whose email never arrived — including one whose first send failed,
    /// since [`Credentials::create_invite`] commits the row before it sends.
    ///
    /// An accepted invite has nothing to resend and is [`Error::NotFound`]; an
    /// expired one is fine, because resending it is the point.
    pub async fn resend_invite(&self, id: Uuid, actor: Uuid) -> Result<UserInvite> {
        let token = OpaqueToken::generate();

        let invite = UserInviteRepository::new(&self.state.pool)
            .rotate_token(id, &token.hash, user_invites::expires_at(Utc::now()))
            .await?
            .ok_or(Error::NotFound)?;

        info!(invite_id = %invite.id, actor_id = %actor, "invite resent");

        self.deliver_invite(&invite, &token.raw).await?;

        Ok(invite)
    }

    /// Create a user out of nothing and sign them in, for `POST /test/users`
    /// (`SPEC.md`, "Test-only routes").
    ///
    /// Behind the same feature gate as the route, because the gate is the
    /// security boundary: in a release build this method does not exist. One
    /// transaction, as login's is — the user and the refresh token it is handed
    /// back with are committed together or not at all — so a user created here
    /// is indistinguishable from one who accepted an invitation, cookie
    /// included.
    ///
    /// No administrator-membership lock is taken: only `PUT` and `DELETE` on
    /// `/users/{id}` can *reduce* the number of administrators (`SPEC.md`,
    /// "Users").
    #[cfg(feature = "integration-tests")]
    pub async fn create_user(
        &self,
        username: Username,
        email: Email,
        password: Password,
        admin: bool,
    ) -> Result<IssuedPair> {
        let new_user = NewUser {
            id: Uuid::new_v4(),
            username,
            email,
            password_hash: hash_blocking(password).await?,
            admin,
            must_change_password: false,
        };

        let mut tx = self.state.pool.begin().await?;
        let user = UserRepository::new(&self.state.pool)
            .insert(&mut tx, &new_user)
            .await?;

        let (user_id, admin) = (user.id, user.admin);
        let pair = self.issue(&mut tx, user).await?;
        tx.commit().await?;

        info!(user_id = %user_id, admin, "test user created");

        Ok(pair)
    }

    /// The cookie that tells a browser to drop its refresh token.
    ///
    /// The route layer asks for this rather than building one: logout sets it
    /// on its 204, and a refresh that answers 401 sets it on the error
    /// response, because a browser holding a token the database will never
    /// accept again should stop sending it (`SPEC.md`, "Authentication"). A 5xx
    /// deliberately does not, which is the caller's decision to make.
    pub fn clearing_cookie(&self) -> Cookie<'static> {
        clear_refresh_cookie(&self.state.config)
    }

    /// Insert a refresh token for `user` in the caller's transaction and build
    /// the pair around it.
    ///
    /// Private, and the only place a refresh token is minted: every caller is
    /// already holding `user`'s row lock (or, for invite acceptance, the invite
    /// row's), has revalidated whatever it is issuing credentials on, and
    /// commits before the returned [`IssuedPair`] reaches a response.
    async fn issue(&self, tx: &mut PgConnection, user: User) -> Result<IssuedPair> {
        let token = OpaqueToken::generate();
        let expires_at = Utc::now() + REFRESH_TOKEN_TTL;

        RefreshTokenRepository::new(&self.state.pool)
            .insert(tx, user.id, &token.hash, expires_at)
            .await?;

        Ok(IssuedPair {
            access_token: self.access_token(&user)?,
            refresh_cookie: refresh_cookie(&self.state.config, &token.raw),
            user,
        })
    }

    /// Sign an access token for `user`, always from a row that is already
    /// committed (`SPEC.md`, "Authentication": "using the current user
    /// values").
    fn access_token(&self, user: &User) -> Result<String> {
        Ok(Claims::for_user(user, Utc::now()).encode(&self.state.config)?)
    }

    /// Send `invite`'s invitation, carrying `raw_token` in the link.
    ///
    /// A failure is logged with the invite id — never the address, the link or
    /// the token (rule 3) — and returned, which [`Error::Email`] renders as a
    /// generic 500. The invite row survives it, so the administrator's next
    /// move is [`Credentials::resend_invite`].
    async fn deliver_invite(&self, invite: &UserInvite, raw_token: &str) -> Result<()> {
        let link = public_link(&self.state.config, INVITE_SEGMENT, raw_token);
        let message = EmailMessage::invitation(&invite.email, &link, invite.expires_at);

        self.state.email.send(message).await.inspect_err(|err| {
            error!(invite_id = %invite.id, error = %err, "the invitation could not be sent");
        })
    }

    /// Count one failed attempt and produce its 401.
    ///
    /// The one place login failure is logged, at `info` with the username and
    /// the address and nothing else: never the password, never the user id of a
    /// row that was found, never which of the two halves was wrong (rule 3).
    fn reject_login(&self, username: &str, client: IpAddr) -> Error {
        self.state.login_throttle.record_failure(username, client);
        info!(username = %username, addr = %client, "login failed");
        Error::Unauthorized(INVALID_CREDENTIALS.to_string())
    }
}

/// The 400 every reset-token rejection shares; see [`INVALID_RESET_TOKEN`].
fn invalid_reset_token() -> Error {
    Error::BadRequest(INVALID_RESET_TOKEN.to_string())
}

/// The 400 both invite paths share; see [`INVALID_INVITE`].
pub(crate) fn invalid_invite() -> Error {
    Error::BadRequest(INVALID_INVITE.to_string())
}

/// The 401 every cookie-authenticated rejection shares.
fn unauthorized() -> Error {
    Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string())
}

/// Verify `candidate` against `hash` on the blocking pool.
///
/// Argon2id at the OWASP parameters is tens of milliseconds of CPU, which is
/// the point of it; running that on a runtime worker would stall every other
/// task on that thread for the duration, so a handful of concurrent logins
/// would stall the whole orchestrator.
///
/// Both arguments are owned because they cross a thread boundary. Neither is
/// ever logged, and the `JoinError` of a panicking task widens into the
/// crate-wide 500 rather than into a failed verification: a check that did not
/// run must not authenticate anyone.
async fn verify_blocking(hash: String, candidate: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || verify_password(&hash, &candidate))
        .await
        .map_err(|err| {
            error!(error = %err, "the password verification task did not finish");
            Error::Internal("password verification failed".to_string())
        })
}

/// Hash `password` on the blocking pool, for the same reason
/// [`verify_blocking`] verifies there.
///
/// Takes a [`Password`] rather than a `String`, so a caller cannot reach this
/// without having applied the 10–128 rule first (`SPEC.md`, "User-facing
/// features") — the validation is the type. The plaintext crosses a thread
/// boundary and is dropped with the closure; `Password` prints a placeholder,
/// so it cannot reach a log line even through a `Debug` field.
///
/// Both failures are 500 and both are already logged where they happen: a
/// panicking task here, and Argon2's own failure inside
/// [`crate::models::user::hash_password`] as [`crate::models::UserError::Hash`].
async fn hash_blocking(password: Password) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_password(password.expose()))
        .await
        .map_err(|err| {
            error!(error = %err, "the password hashing task did not finish");
            Error::Internal("password hashing failed".to_string())
        })?
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The password [`DUMMY_PASSWORD_HASH`] was generated from. Obviously
    /// fake, and used only to prove the constant is a real Argon2id PHC
    /// string: a truncated or misquoted one would silently fail to parse, the
    /// verification would return in microseconds and the timing defence would
    /// be gone without any test noticing.
    const DUMMY_PASSWORD: &str = "not-a-real-password";

    #[test]
    fn the_dummy_hash_is_a_working_argon2id_hash() {
        assert!(verify_password(DUMMY_PASSWORD_HASH, DUMMY_PASSWORD));
        assert!(!verify_password(DUMMY_PASSWORD_HASH, "something-else"));
    }

    #[test]
    fn no_row_can_ever_carry_the_dummy_hash() {
        // It is a constant in this file and nothing writes it, but the salt is
        // what makes that structural: hashing the same password again gives a
        // different PHC string, so no user who happened to choose
        // `not-a-real-password` would end up matching it.
        let other = crate::models::user::hash_password(DUMMY_PASSWORD).expect("hashing succeeds");
        assert_ne!(other, DUMMY_PASSWORD_HASH);
    }

    #[tokio::test]
    async fn verification_on_the_blocking_pool_agrees_with_the_model() {
        assert!(
            verify_blocking(DUMMY_PASSWORD_HASH.to_string(), DUMMY_PASSWORD.to_string())
                .await
                .expect("the task finishes")
        );
        assert!(
            !verify_blocking(DUMMY_PASSWORD_HASH.to_string(), "wrong".to_string())
                .await
                .expect("the task finishes")
        );
        // A hash this build cannot parse is a failed verification, not an
        // error the login handler has to distinguish.
        assert!(
            !verify_blocking(
                "$argon2id$fake$hash".to_string(),
                DUMMY_PASSWORD.to_string()
            )
            .await
            .expect("the task finishes")
        );
    }
}
