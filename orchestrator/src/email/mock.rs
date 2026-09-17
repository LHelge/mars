//! The email mock, compiled only with the `integration-tests` feature.
//!
//! Invite and password-reset flows are asserted through the messages it
//! captured (`CLAUDE.md`, "Testing expectations").

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::{EmailClient, EmailError, EmailMessage};
use crate::prelude::*;

/// The status the simulated provider failure reports.
const SIMULATED_PROVIDER_STATUS: u16 = 500;

/// A client that delivers nowhere and keeps every message in order.
#[derive(Debug, Default)]
pub struct MockEmailClient {
    sent: Mutex<Vec<EmailMessage>>,
    fail_next: AtomicBool,
}

impl MockEmailClient {
    /// A fresh mock with no captured messages.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every message sent so far, in order. Cloned out of the mutex, so a test
    /// never holds the lock across an await.
    pub fn sent(&self) -> Vec<EmailMessage> {
        self.lock().clone()
    }

    /// Forget everything captured so far.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Make exactly the next send fail, as a provider refusal would.
    ///
    /// The failed message is not captured, so `sent()` still reflects what a
    /// caller actually got delivered.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn lock(&self) -> MutexGuard<'_, Vec<EmailMessage>> {
        // Test-only code: a poisoned lock means another test thread already
        // panicked, which is a failure in its own right.
        self.sent.lock().expect("the mock email lock is healthy")
    }
}

#[async_trait]
impl EmailClient for MockEmailClient {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn send(&self, message: EmailMessage) -> Result<()> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(EmailError::Provider {
                status: SIMULATED_PROVIDER_STATUS,
            }
            .into());
        }

        self.lock().push(message);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(subject: &str) -> EmailMessage {
        EmailMessage {
            to: "someone@example.test".to_string(),
            subject: subject.to_string(),
            text: "https://mars.example.invalid/invite/not-a-real-token".to_string(),
        }
    }

    #[tokio::test]
    async fn messages_are_captured_in_send_order() {
        let client = MockEmailClient::new();
        assert!(client.sent().is_empty());

        client
            .send(message("first"))
            .await
            .expect("the mock always accepts");
        client
            .send(message("second"))
            .await
            .expect("the mock always accepts");

        let subjects: Vec<String> = client.sent().into_iter().map(|m| m.subject).collect();
        assert_eq!(subjects, vec!["first".to_string(), "second".to_string()]);

        client.clear();
        assert!(client.sent().is_empty());
    }

    #[tokio::test]
    async fn fail_next_fails_exactly_one_send() {
        let client = MockEmailClient::new();
        client.fail_next();

        let error = client
            .send(message("refused"))
            .await
            .expect_err("the armed send fails");
        assert!(matches!(
            error,
            Error::Email(EmailError::Provider { status: 500 })
        ));
        assert!(client.sent().is_empty(), "a failed send is not captured");

        client
            .send(message("delivered"))
            .await
            .expect("only one send was armed to fail");
        assert_eq!(client.sent().len(), 1);
    }

    #[tokio::test]
    async fn as_any_downcasts_a_trait_object_back_to_the_mock() {
        let client: Arc<dyn EmailClient> = Arc::new(MockEmailClient::new());
        client
            .send(message("invitation"))
            .await
            .expect("the mock always accepts");

        let mock = client
            .as_any()
            .downcast_ref::<MockEmailClient>()
            .expect("the trait object is the mock");
        assert_eq!(mock.sent().len(), 1);
    }
}
