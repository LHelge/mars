//! The per-session MCP bearer token: generation, hashing and redaction.
//!
//! One token belongs to one session *process*, not to the session: every actual
//! launch, resume and conversational retry generates a fresh one, the database
//! keeps only its SHA-256 and the raw value exists on disk in that session's
//! `mcp.json` and nowhere else (`ARCHITECTURE.md`, "MCP design" →
//! "Per-session config file"; `docs/data-model.md`, `sessions.mcp_token_hash`;
//! ADR 0029). The hash cannot be turned back into the token, so a launch whose
//! preparation was interrupted generates another token rather than recovering
//! the old one.
//!
//! [`hash_mcp_token`] is the one function both ends use: the launcher hashes
//! the token it generated on the way into the row, and the MCP server's bearer
//! middleware hashes the token a request presented on the way back in, so the
//! comparison is always between two stored-shaped hashes and never against a
//! live credential (`ARCHITECTURE.md`, "Trust boundaries").
//!
//! Rule 3 of `CLAUDE.md` is why [`McpToken`] has a hand-written [`Debug`], is
//! zeroized when it drops and never implements `Display`, `Serialize` or
//! `Deref<Target = str>`: the only way to the raw bytes is
//! [`McpToken::expose`], which is called by
//! [`write_mcp_json`](super::write_mcp_json) and by nothing else.

use rand::Rng;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::models::OpaqueToken;

/// How many random bytes a token carries: 256 bits, the same strength as the
/// opaque tokens of the auth flows (`crate::models::OpaqueToken`).
const TOKEN_BYTES: usize = 32;

/// Length of a token as it is written: 32 bytes in URL-safe base64 without
/// padding, which is `ceil(32 * 4 / 3) = 43` characters.
///
/// URL-safe and unpadded so the value passes through an `Authorization` header,
/// a JSON document and a shell without quoting or escaping.
pub const MCP_TOKEN_CHARS: usize = 43;

/// The lowercase hex SHA-256 of a raw MCP bearer token.
///
/// The one hashing function for `sessions.mcp_token_hash`: the launcher calls
/// it through [`McpToken::hash`] before inserting or updating the row, and the
/// MCP bearer middleware calls it on the token a request presented and looks
/// the session up by the result. It is
/// [`OpaqueToken::hash_of`](crate::models::OpaqueToken::hash_of) — the same
/// SHA-256 and the same hex rendering every other token hash in the schema
/// uses — under the name the MCP layer knows it by, so the two cannot drift.
pub fn hash_mcp_token(raw: &str) -> String {
    OpaqueToken::hash_of(raw)
}

/// A freshly generated MCP bearer token, on its way to one session's
/// `mcp.json`.
///
/// Holding one means holding a live credential: it is zeroized when it drops,
/// its `Debug` prints nothing, and it is moved rather than cloned so there is
/// one copy to lose track of.
#[derive(PartialEq, Eq)]
pub struct McpToken {
    /// The raw token, for `mcp.json` only — never for a row, a response or a
    /// log line.
    raw: String,
}

impl McpToken {
    /// A new token: [`TOKEN_BYTES`] cryptographically random bytes in URL-safe
    /// base64 without padding.
    ///
    /// The randomness comes from `rand::rng()`, the thread-local CSPRNG the
    /// secrets module already generates data keys and nonces with
    /// (`crate::secrets::crypto`); it is seeded and periodically reseeded from
    /// the operating system's generator (`rand::rngs::SysRng`, which is what
    /// `rand` 0.10 renamed `OsRng` to). Going through it rather than reading
    /// the system generator directly keeps generation infallible, which is
    /// what lets the launcher treat a token as a value rather than a fallible
    /// step.
    pub fn generate() -> Self {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use zeroize::Zeroizing;

        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        rand::rng().fill_bytes(bytes.as_mut_slice());

        Self {
            raw: URL_SAFE_NO_PAD.encode(&bytes[..]),
        }
    }

    /// The lowercase hex SHA-256 that goes into `sessions.mcp_token_hash`.
    ///
    /// The hash is not a credential — it cannot be presented to the MCP server
    /// — so unlike the token itself it may be passed to a repository and
    /// compared.
    pub fn hash(&self) -> String {
        hash_mcp_token(&self.raw)
    }

    /// The raw token, for the one writer that has to put it in a file.
    ///
    /// Every other caller wants [`McpToken::hash`]. Deliberately verbose: a
    /// call to this is the place to ask whether the value is about to reach a
    /// log line, a response body or an event payload (rule 3).
    pub fn expose(&self) -> &str {
        &self.raw
    }

    /// A token with a fixed value, so a test can assert the bytes of a document
    /// it appears in.
    ///
    /// Unit tests only; a production caller has no reason to choose a token.
    #[cfg(test)]
    pub(crate) fn from_raw_for_test(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
        }
    }
}

impl Zeroize for McpToken {
    fn zeroize(&mut self) {
        self.raw.zeroize();
    }
}

impl Drop for McpToken {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Promised by the [`Drop`] above; `zeroize`'s derive feature is not enabled,
/// so both halves are written by hand.
impl ZeroizeOnDrop for McpToken {}

impl std::fmt::Debug for McpToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpToken(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_forty_three_url_safe_base64_characters() {
        let token = McpToken::generate();
        let raw = token.expose();

        assert_eq!(raw.len(), MCP_TOKEN_CHARS, "token was {}", raw.len());
        assert!(
            raw.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "not URL-safe base64: {raw}"
        );
        assert!(!raw.contains('='), "padded: {raw}");
    }

    #[test]
    fn the_hash_is_sixty_four_lowercase_hex_characters() {
        let hash = McpToken::generate().hash();

        assert_eq!(hash.len(), crate::models::TOKEN_HASH_CHARS);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "not lowercase hex: {hash}"
        );
    }

    #[test]
    fn the_free_function_is_the_method() {
        let token = McpToken::generate();
        assert_eq!(hash_mcp_token(token.expose()), token.hash());
    }

    #[test]
    fn hashing_is_deterministic_and_the_documented_sha256() {
        // The published SHA-256 of "abc", which pins the hex rendering the
        // bearer middleware will compare stored rows against.
        assert_eq!(
            hash_mcp_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash_mcp_token("abc"), hash_mcp_token("abc"));
    }

    #[test]
    fn two_tokens_differ() {
        let one = McpToken::generate();
        let two = McpToken::generate();

        assert_ne!(one.expose(), two.expose());
        assert_ne!(one.hash(), two.hash());
    }

    #[test]
    fn the_hash_does_not_contain_the_token() {
        let token = McpToken::generate();
        assert!(!token.hash().contains(token.expose()));
    }

    #[test]
    fn debug_is_redacted_even_nested() {
        let token = McpToken::generate();

        let rendered = format!("{token:?}");
        assert_eq!(rendered, "McpToken(<redacted>)");
        assert!(!rendered.contains(token.expose()), "leaked: {rendered}");

        // And inside somebody else's derived `Debug`.
        let rendered = format!("{:?}", Some(("mcp", &token)));
        assert!(!rendered.contains(token.expose()), "leaked: {rendered}");
    }

    #[test]
    fn zeroize_clears_the_raw_token() {
        let mut token = McpToken::generate();
        token.zeroize();

        assert!(token.expose().is_empty(), "not cleared: {:?}", token);
    }
}
