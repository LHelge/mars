//! The `EmailClient` trait, the Resend implementation, the log fallback and
//! the mock behind the `integration-tests` feature.
//!
//! Three messages leave the orchestrator in v1 (ADR 0014, `ARCHITECTURE.md`,
//! "Task tracker"): an invitation, a password reset and a task escalation.
//! Each is built by one of the [`EmailMessage`] constructors here, so the
//! wording and the subject live in one place and the flows only supply the
//! link.
//!
//! **Delivery.** [`ResendClient`] when `RESEND_API_KEY` is set, otherwise
//! [`LogEmailClient`], chosen once at startup (`README.md`, "Configuration").
//! The fallback writes the complete message, link included, to the operator's
//! log at `info`: the one sanctioned exception to the secret-logging rule
//! (`CLAUDE.md` rule 3, ADR 0026, `SPEC.md`, "Users"). `ResendClient` never
//! logs the text, and a provider failure is a failure — it does not fall back
//! to logging.
//!
//! **Async style.** `#[async_trait::async_trait]`, for the reason given in
//! [`crate::engine`]: the trait is held as `Arc<dyn EmailClient>` and must
//! stay dyn compatible.

use std::any::Any;

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};

use crate::prelude::*;

pub mod log;
pub mod resend;

#[cfg(feature = "integration-tests")]
pub mod mock;

pub use log::LogEmailClient;
pub use resend::ResendClient;

/// How an expiry is written into a message body. Deliberately not RFC 3339:
/// the API speaks that, a person reading their invitation does not.
const EXPIRY_FORMAT: &str = "%Y-%m-%d %H:%M UTC";

/// One outgoing message.
///
/// Plain text only: every v1 message is a sentence and a link. Never logged by
/// this module — the one documented exception is [`LogEmailClient`] (rule 3,
/// ADR 0026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
    /// The recipient address.
    pub to: String,
    /// The subject line.
    pub subject: String,
    /// The body.
    pub text: String,
}

impl EmailMessage {
    /// The invitation an admin sends from the admin page.
    ///
    /// `link` is the finished `<PUBLIC_URL>/invite/<token>`; the caller builds
    /// it, because only the caller holds the token (`SPEC.md`, "Users").
    pub fn invitation(to: &str, link: &str, expires_at: DateTime<Utc>) -> Self {
        let expires = expires_at.format(EXPIRY_FORMAT);
        Self {
            to: to.to_string(),
            subject: "You have been invited to Mars".to_string(),
            text: format!(
                "You have been invited to Mars.\n\
                 \n\
                 Open this link to choose a password and sign in:\n\
                 \n\
                 {link}\n\
                 \n\
                 The link can be used once and expires on {expires}.\n"
            ),
        }
    }

    /// The password reset a user asks for from the login page.
    ///
    /// `link` is the finished `<PUBLIC_URL>/reset-password/<token>`.
    pub fn password_reset(to: &str, link: &str, expires_at: DateTime<Utc>) -> Self {
        let expires = expires_at.format(EXPIRY_FORMAT);
        Self {
            to: to.to_string(),
            subject: "Reset your Mars password".to_string(),
            text: format!(
                "A password reset was requested for your Mars account.\n\
                 \n\
                 Open this link to choose a new password:\n\
                 \n\
                 {link}\n\
                 \n\
                 The link can be used once and expires on {expires}.\n\
                 \n\
                 If you did not ask for this, ignore this message. Your \
                 password stays as it is.\n"
            ),
        }
    }

    /// The escalation sent when a task moves into the human state, by the
    /// `needs_human` tool or by the reaper (`ARCHITECTURE.md`, "Task
    /// tracker", "Notification").
    ///
    /// Names the project, the task and the reason, and carries the finished
    /// `<PUBLIC_URL>/projects/<project_id>/tasks/<number>` link. Recipients
    /// who turned `notify_email` off are filtered by the caller, not here.
    pub fn escalation(
        to: &str,
        project_name: &str,
        task_number: i32,
        task_title: &str,
        reason: &str,
        link: &str,
    ) -> Self {
        Self {
            to: to.to_string(),
            subject: format!("Task #{task_number} needs a human: {task_title}"),
            text: format!(
                "Task #{task_number} in {project_name} needs a human.\n\
                 \n\
                 {task_title}\n\
                 \n\
                 Reason: {reason}\n\
                 \n\
                 {link}\n"
            ),
        }
    }
}

/// Anything that stops a message going out.
#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    /// The message or the mail configuration cannot produce a delivery: an
    /// empty recipient, or a sender startup did not supply.
    #[error("the message cannot be delivered: {0}")]
    Configuration(String),
    /// The provider could not be reached. The string is the HTTP client's own
    /// report and never carries the API key or the message text.
    #[error("the email provider could not be reached: {0}")]
    Transport(String),
    /// The provider answered, and refused. Only the status is kept: the
    /// response body is never read into an error, a log or a response.
    #[error("the email provider refused the message: HTTP {status}")]
    Provider {
        /// The provider's HTTP status.
        status: u16,
    },
}

impl EmailError {
    /// The HTTP status this failure maps to. A caller never gets to choose the
    /// mail provider, so every failure here is an internal fault.
    pub fn status(&self) -> StatusCode {
        match self {
            EmailError::Configuration(_)
            | EmailError::Transport(_)
            | EmailError::Provider { .. } => StatusCode::INTERNAL_SERVER_ERROR,
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

/// Refuse a message nothing could deliver, before any provider call.
///
/// Private to the module and used by both real clients; the mock captures
/// whatever it is handed, so a test can assert on it.
fn check_recipient(message: &EmailMessage) -> std::result::Result<(), EmailError> {
    if message.to.trim().is_empty() {
        return Err(EmailError::Configuration(
            "the message has no recipient".to_string(),
        ));
    }
    Ok(())
}

/// An in-memory `tracing` writer, so a test can assert both what a client
/// logged and what it did not.
#[cfg(test)]
pub(crate) mod capture {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing::Level;
    use tracing::subscriber::Subscriber;
    use tracing_subscriber::fmt::MakeWriter;

    /// A cloneable handle onto one shared buffer.
    #[derive(Clone, Default)]
    pub(crate) struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        /// An empty buffer.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Everything written so far.
        pub(crate) fn contents(&self) -> String {
            let buffer = self.buffer.lock().expect("the capture lock is healthy");
            String::from_utf8_lossy(&buffer).into_owned()
        }

        /// A subscriber that records every level into this buffer.
        pub(crate) fn subscriber(&self) -> impl Subscriber + Send + Sync + 'static {
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_max_level(Level::TRACE)
                .with_writer(self.clone())
                .finish()
        }
    }

    impl io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut buffer = self.buffer.lock().expect("the capture lock is healthy");
            buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// Obviously fake tokens; nothing here is a credential (rule 3).
    const INVITE_LINK: &str = "https://mars.example.invalid/invite/not-a-real-token";
    const RESET_LINK: &str = "https://mars.example.invalid/reset-password/not-a-real-token";

    fn expiry() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
            .single()
            .expect("a valid instant")
    }

    #[test]
    fn the_invitation_carries_its_link_and_expiry() {
        let message = EmailMessage::invitation("someone@example.test", INVITE_LINK, expiry());

        assert_eq!(message.to, "someone@example.test");
        assert_eq!(message.subject, "You have been invited to Mars");
        assert!(message.text.contains(INVITE_LINK), "{}", message.text);
        assert!(
            message.text.contains("2026-03-04 05:06 UTC"),
            "{}",
            message.text
        );
    }

    #[test]
    fn the_password_reset_carries_its_link_and_expiry() {
        let message = EmailMessage::password_reset("someone@example.test", RESET_LINK, expiry());

        assert_eq!(message.subject, "Reset your Mars password");
        assert!(message.text.contains(RESET_LINK), "{}", message.text);
        assert!(
            message.text.contains("2026-03-04 05:06 UTC"),
            "{}",
            message.text
        );
    }

    #[test]
    fn the_escalation_names_the_project_task_reason_and_link() {
        let link =
            "https://mars.example.invalid/projects/00000000-0000-0000-0000-000000000001/tasks/17";
        let message = EmailMessage::escalation(
            "someone@example.test",
            "Apollo",
            17,
            "Rotate the deploy key",
            "the agent could not reach the registry",
            link,
        );

        assert_eq!(
            message.subject,
            "Task #17 needs a human: Rotate the deploy key"
        );
        assert!(message.text.contains("Apollo"), "{}", message.text);
        assert!(message.text.contains("#17"), "{}", message.text);
        assert!(
            message.text.contains("Rotate the deploy key"),
            "{}",
            message.text
        );
        assert!(
            message
                .text
                .contains("the agent could not reach the registry"),
            "{}",
            message.text
        );
        assert!(message.text.contains(link), "{}", message.text);
    }

    #[test]
    fn every_failure_is_an_internal_fault() {
        for error in [
            EmailError::Configuration("no recipient".to_string()),
            EmailError::Transport("connection refused".to_string()),
            EmailError::Provider { status: 422 },
        ] {
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[test]
    fn a_blank_recipient_is_a_configuration_failure() {
        let message = EmailMessage::invitation("   ", INVITE_LINK, expiry());
        let error = check_recipient(&message).expect_err("a blank recipient is refused");
        assert!(matches!(error, EmailError::Configuration(_)));

        let message = EmailMessage::invitation("someone@example.test", INVITE_LINK, expiry());
        assert!(check_recipient(&message).is_ok());
    }
}
