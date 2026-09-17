//! The delivery path used when no email provider is configured.
//!
//! Selected automatically when `RESEND_API_KEY` is unset (ADR 0014,
//! `README.md`, "Configuration"). It writes the whole message, including the
//! usable token-bearing link, to the operator's log at `info` so a local
//! deployment can complete an invitation or a password reset by copying the
//! link out of the log. That is deliberate and is the single exception to the
//! secret-logging rule (`CLAUDE.md` rule 3, ADR 0026, `SPEC.md`, "Users"); no
//! other code path may log a token, and a failure of the real provider never
//! falls back to this client.

use std::any::Any;

use async_trait::async_trait;

use super::{EmailClient, EmailMessage, check_recipient};
use crate::prelude::*;

/// Writes every message to the log instead of sending it.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogEmailClient;

impl LogEmailClient {
    /// The only constructor; the client holds nothing.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EmailClient for LogEmailClient {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn send(&self, message: EmailMessage) -> Result<()> {
        check_recipient(&message)?;

        // The sanctioned exception: `text` carries the link and its token on
        // purpose (ADR 0026).
        info!(
            to = %message.to,
            subject = %message.subject,
            text = %message.text,
            "email delivery is not configured; message written to the log (ADR 0026)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use tracing::instrument::WithSubscriber;

    use super::*;
    use crate::email::EmailError;
    use crate::email::capture::CaptureWriter;

    /// An obviously fake token (rule 3).
    const LINK: &str = "https://mars.example.invalid/invite/not-a-real-token";

    fn invitation(to: &str) -> EmailMessage {
        let expires = Utc
            .with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
            .single()
            .expect("a valid instant");
        EmailMessage::invitation(to, LINK, expires)
    }

    #[tokio::test]
    async fn the_whole_message_including_the_link_is_logged() {
        let writer = CaptureWriter::new();

        LogEmailClient::new()
            .send(invitation("someone@example.test"))
            .with_subscriber(writer.subscriber())
            .await
            .expect("the log client always accepts a deliverable message");

        let logged = writer.contents();
        assert!(logged.contains("someone@example.test"), "{logged}");
        assert!(logged.contains("You have been invited to Mars"), "{logged}");
        assert!(logged.contains(LINK), "{logged}");
        assert!(logged.contains("ADR 0026"), "{logged}");
    }

    #[tokio::test]
    async fn a_blank_recipient_is_refused_and_nothing_is_logged() {
        let writer = CaptureWriter::new();

        let error = LogEmailClient::new()
            .send(invitation("  "))
            .with_subscriber(writer.subscriber())
            .await
            .expect_err("a message with no recipient is refused");

        assert!(matches!(error, Error::Email(EmailError::Configuration(_))));
        assert!(!writer.contents().contains(LINK), "{}", writer.contents());
    }
}
