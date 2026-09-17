//! The opaque token every non-JWT credential is made of.
//!
//! Refresh tokens, invite tokens and password-reset tokens are all the same
//! shape (`docs/data-model.md`, `refresh_tokens`, `user_invites`,
//! `password_reset_tokens`): the raw token is two version-4 UUIDs joined by a
//! `.`, and the row stores its SHA-256 hex. That is 256 bits of randomness in
//! a form that survives a URL and a cookie without escaping, and a database
//! dump of the token tables cannot be replayed.
//!
//! The raw half exists exactly once, on its way out in a cookie or an email
//! link; only [`OpaqueToken::hash`] is ever stored, and it is the same function
//! ([`OpaqueToken::hash_of`]) that a presented token is put through on the way
//! back in.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Length of a SHA-256 digest rendered as lowercase hex.
pub const TOKEN_HASH_CHARS: usize = 64;

/// A freshly generated opaque token: what is handed out, and what is stored.
///
/// `Debug` is written by hand rather than derived: a `{:?}` of this struct, or
/// of anything holding one, would otherwise put a live credential in a log
/// line (`CLAUDE.md`, rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct OpaqueToken {
    /// The raw token, for the cookie or the email link only — never for a row
    /// and never for a log line.
    pub raw: String,
    /// The lowercase SHA-256 hex that goes in `token_hash`.
    pub hash: String,
}

impl OpaqueToken {
    /// A new token: `<uuid v4>.<uuid v4>` and its SHA-256 hex.
    pub fn generate() -> Self {
        let raw = format!("{}.{}", Uuid::new_v4(), Uuid::new_v4());
        let hash = Self::hash_of(&raw);
        Self { raw, hash }
    }

    /// The lowercase SHA-256 hex of `raw`.
    ///
    /// The same function on the way out and on the way in, so a presented
    /// token is looked up by hashing it and comparing stored bytes; the raw
    /// token itself is never compared against anything.
    pub fn hash_of(raw: &str) -> String {
        use std::fmt::Write as _;

        let digest = Sha256::digest(raw.as_bytes());
        let mut hex = String::with_capacity(TOKEN_HASH_CHARS);
        for byte in digest.iter() {
            // Infallible: writing to a `String` never fails.
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}

impl std::fmt::Debug for OpaqueToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The hash is not a secret, but printing it would let a log line be
        // matched against a stored row, so neither half is shown.
        f.write_str("OpaqueToken(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_token_is_two_uuids_joined_by_a_dot() {
        let token = OpaqueToken::generate();

        let (left, right) = token
            .raw
            .split_once('.')
            .expect("the raw token has one dot");
        assert!(!right.contains('.'), "more than one dot: {}", token.raw);

        for half in [left, right] {
            let parsed = Uuid::parse_str(half).expect("each half is a UUID");
            assert_eq!(parsed.get_version_num(), 4, "half {half} is not v4");
            // Hyphenated lowercase, which is what `Display` produces.
            assert_eq!(parsed.to_string(), half);
        }
    }

    #[test]
    fn the_hash_is_sixty_four_lowercase_hex_characters() {
        let token = OpaqueToken::generate();

        assert_eq!(token.hash.len(), TOKEN_HASH_CHARS);
        assert!(
            token
                .hash
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "not lowercase hex: {}",
            token.hash
        );
    }

    #[test]
    fn the_stored_hash_is_what_hashing_the_raw_token_produces() {
        let token = OpaqueToken::generate();
        assert_eq!(OpaqueToken::hash_of(&token.raw), token.hash);
    }

    #[test]
    fn hashing_matches_the_known_sha256_of_a_fixed_string() {
        // The published SHA-256 of "abc" and of the empty string; this pins the
        // hex rendering rather than only its self-consistency.
        assert_eq!(
            OpaqueToken::hash_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            OpaqueToken::hash_of(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn two_tokens_differ() {
        let one = OpaqueToken::generate();
        let two = OpaqueToken::generate();

        assert_ne!(one.raw, two.raw);
        assert_ne!(one.hash, two.hash);
    }

    #[test]
    fn the_hash_does_not_reveal_the_raw_token() {
        let token = OpaqueToken::generate();
        assert!(!token.hash.contains('.'), "{}", token.hash);
        assert!(!token.hash.contains(&token.raw));
    }

    #[test]
    fn debug_never_shows_either_half() {
        let token = OpaqueToken::generate();
        let rendered = format!("{token:?}");

        assert_eq!(rendered, "OpaqueToken(<redacted>)");
        assert!(!rendered.contains(&token.raw), "leaked: {rendered}");
        assert!(!rendered.contains(&token.hash), "leaked: {rendered}");

        // And nested inside something else's derived `Debug`.
        let rendered = format!("{:?}", Some(("refresh", &token)));
        assert!(!rendered.contains(&token.raw), "leaked: {rendered}");
    }
}
