//! The `rmcp` handler the Streamable HTTP service builds one of per MCP
//! session (`ARCHITECTURE.md`, "MCP design").
//!
//! It holds nothing session-specific: the factory in [`super::mcp_router`]
//! clones [`AppState`] and nothing else, and everything about the *calling*
//! session comes from the per-request [`SessionContext`]. That split is what
//! lets the bearer middleware run per HTTP request while the MCP session,
//! which is keyed by `Mcp-Session-Id` and outlives a token rotation, stays
//! ignorant of tokens.
//!
//! `list_tools` and `call_tool` are the dispatch task's; until then the
//! defaults `rmcp` provides answer an empty list and "method not found".

use rmcp::ServerHandler;
use rmcp::model::{ErrorData, Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::{RequestContext, RoleServer};

use crate::prelude::*;

use super::SessionContext;

/// How the server names itself in `initialize` (`ARCHITECTURE.md`, "MCP
/// design", Per-session config file: the same name the generated `mcp.json`
/// gives the server, chosen over the bare `mars` so a repository's own
/// `.mcp.json` is unlikely to shadow it).
const SERVER_NAME: &str = "mars-orchestrator";

/// The one-paragraph brief the CLI puts in front of the model. Short and
/// opinionated on purpose (`ARCHITECTURE.md`, "MCP design", Tool exposure).
const INSTRUCTIONS: &str = "Mars task tracker and git tools. Call ready to find work, claim before working, comment before handing off.";

/// What the server advertises in its `initialize` response.
///
/// A free function rather than a method because it reads nothing from the
/// state: the identity, the one capability and the instructions are constants,
/// and keeping them out of `self` is what lets a unit test assert on them
/// without building an [`AppState`].
fn server_info() -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
}

/// The MCP handler. One per MCP session, built by the factory closure in
/// [`super::mcp_router`].
#[derive(Clone)]
pub struct McpServer {
    #[allow(dead_code, reason = "the tool handlers of the dispatch task read it")]
    state: AppState,
}

impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The calling session, as the bearer middleware resolved it.
    ///
    /// **How the pinned `rmcp` forwards HTTP request data.** Verified against
    /// `rmcp` 3.4.0: `StreamableHttpService` consumes the request body and
    /// inserts the remaining `http::request::Parts` — extensions included —
    /// into the `RequestContext`'s own extensions, and nothing else of the
    /// HTTP request travels with the MCP message (see that type's "Accessing
    /// HTTP request data from tool handlers", and the
    /// `extensions_mut().insert(part)` calls on every dispatch path in
    /// `transport::streamable_http_server::tower`). So an `axum` middleware
    /// that inserts a [`SessionContext`] into the *request* extensions is read
    /// back here through `Parts`, and not directly off `ctx.extensions`.
    ///
    /// The direct lookup is tried first all the same: it costs one map probe,
    /// it is what a future `rmcp` that flattens the extensions would populate,
    /// and it is how a test can supply a context without an HTTP request at
    /// all.
    ///
    /// A missing context means the middleware did not run — a wiring fault,
    /// not a caller's mistake — so it is logged and answered as an internal
    /// error rather than panicking the session's whole connection.
    #[allow(
        dead_code,
        reason = "the bearer-auth and tool-dispatch tasks are its callers"
    )]
    // Not the crate's `Result` alias: an MCP handler answers with `ErrorData`,
    // which the JSON-RPC envelope carries, rather than with the HTTP `Error`.
    fn context(
        &self,
        ctx: &RequestContext<RoleServer>,
    ) -> std::result::Result<SessionContext, ErrorData> {
        let resolved = ctx
            .extensions
            .get::<SessionContext>()
            .or_else(|| {
                ctx.extensions
                    .get::<axum::http::request::Parts>()
                    .and_then(|parts| parts.extensions.get::<SessionContext>())
            })
            .cloned();

        match resolved {
            Some(context) => {
                debug!(session_id = %context.session_id, "mcp request context resolved");
                Ok(context)
            }
            None => {
                error!("mcp request without session context");
                Err(ErrorData::internal_error("internal error", None))
            }
        }
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        server_info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_names_itself_as_the_generated_mcp_json_does() {
        assert_eq!(server_info().server_info.name, "mars-orchestrator");
    }

    #[test]
    fn the_instructions_are_the_documented_brief() {
        assert_eq!(
            server_info().instructions.as_deref(),
            Some(
                "Mars task tracker and git tools. Call ready to find work, claim before working, comment before handing off."
            )
        );
    }

    #[test]
    fn tools_are_the_only_capability_advertised() {
        let capabilities = server_info().capabilities;

        assert!(capabilities.tools.is_some(), "tools are advertised");
        assert!(capabilities.prompts.is_none());
        assert!(capabilities.resources.is_none());
        assert!(capabilities.logging.is_none());
        assert!(capabilities.completions.is_none());
        assert!(capabilities.experimental.is_none());
        assert!(capabilities.extensions.is_none());
    }
}
