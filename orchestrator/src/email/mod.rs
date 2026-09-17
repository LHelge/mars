//! The `EmailClient` trait, the Resend implementation, the log fallback and
//! the mock behind the `integration-tests` feature.
//!
//! Only the shape the test harness depends on exists yet: the trait, an
//! [`EmailMessage`], an [`EmailError`] and a startup placeholder. The
//! authentication epic adds `ResendClient` and `LogEmailClient` (ADR 0026)
//! behind this same trait, and the invite and reset flows that build the
//! messages.
//!
//! **Async style.** `#[async_trait::async_trait]`, for the reason given in
//! [`crate::engine`]: the trait is held as `Arc<dyn EmailClient>` and must
//! stay dyn compatible.

use std::any::Any;

use async_trait::async_trait;
use axum::http::StatusCode;

use crate::prelude::*;

#[cfg(feature = "integration-tests")]
pub mod mock;

/// One outgoing message.
///
/// Plain text only: v1 sends invitations and password resets, both of which
/// are a sentence and a link. Never logged by this module — the one documented
/// exception is `LogEmailClient` (rule 3, ADR 0026), which the authentication
/// epic adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
    /// The recipient address.
    pub to: String,
    /// The subject line.
    pub subject: String,
    /// The body.
    pub text: String,
}

/// Anything that stops a message going out.
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// No mail client is wired up yet.
    #[error("no email client is configured")]
    NotConfigured,
    /// The provider refused the message or could not be reached. The string is
    /// the provider's own report and never carries the API key.
    #[error("the email provider failed: {0}")]
    Transport(String),
}

impl EmailError {
    /// The HTTP status this failure maps to. A caller never gets to choose the
    /// mail provider, so every failure here is an internal fault.
    pub fn status(&self) -> StatusCode {
        match self {
            EmailError::NotConfigured | EmailError::Transport(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// Outgoing mail, behind a trait so invite flows can be asserted without a
/// provider (`CLAUDE.md`, "Testing expectations").
#[async_trait]
pub trait EmailClient: Send + Sync {
    /// Downcast hook, so a test can reach `mock::MockEmailClient::sent`.
    fn as_any(&self) -> &dyn Any;

    /// Deliver one message.
    async fn send(&self, message: EmailMessage) -> Result<()>;
}

/// The client used until the authentication epic wires up Resend and the log
/// fallback.
///
/// It refuses every message rather than dropping it silently, so a flow that
/// starts sending mail before its client exists fails loudly.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceholderEmailClient;

#[async_trait]
impl EmailClient for PlaceholderEmailClient {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn send(&self, _message: EmailMessage) -> Result<()> {
        Err(EmailError::NotConfigured.into())
    }
}
