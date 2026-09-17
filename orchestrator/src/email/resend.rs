//! Production delivery: the Resend HTTP API over `reqwest` (ADR 0014).
//!
//! One `POST` per message to `https://api.resend.com/emails` with
//! `Authorization: Bearer <RESEND_API_KEY>` and a
//! `{ "from", "to": [..], "subject", "text" }` body. Nothing here logs the
//! message text or the link at any level, and a refusal is an error rather
//! than a reason to fall back to the log client (ADR 0026): the recipient,
//! the subject and the provider's status are all that ever reach the log.

use std::any::Any;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use super::{EmailClient, EmailError, EmailMessage, check_recipient};
use crate::prelude::*;

/// Where Resend accepts messages.
const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

/// How long one delivery may take. Mail is sent inside a request handler, so a
/// provider that stops answering must not hold the caller open.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The Resend client.
///
/// Built once at startup and shared through `Arc<dyn EmailClient>`; the inner
/// `reqwest::Client` carries the connection pool, so it is built once too.
#[derive(Clone)]
pub struct ResendClient {
    http: reqwest::Client,
    api_key: String,
    from: String,
    base_url: String,
}

impl fmt::Debug for ResendClient {
    /// Never prints the API key (rule 3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResendClient")
            .field("from", &self.from)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl ResendClient {
    /// A client that posts to Resend with `api_key` and sends as `from`.
    ///
    /// Called from startup, which is where the `expect` is allowed: a
    /// `reqwest::Client` that cannot be built means the process has no TLS
    /// backend at all and nothing downstream could recover from it.
    pub fn new(api_key: String, from: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("the HTTP client builds");

        Self {
            http,
            api_key,
            from,
            base_url: RESEND_ENDPOINT.to_string(),
        }
    }

    /// Point the client at another URL.
    ///
    /// Tests only: it lets a local fake stand in for Resend so no unit test
    /// touches the network. Production always uses [`RESEND_ENDPOINT`].
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

/// The Resend request body.
#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    text: &'a str,
}

#[async_trait]
impl EmailClient for ResendClient {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn send(&self, message: EmailMessage) -> Result<()> {
        check_recipient(&message)?;

        // `to` and `subject` only: the body carries the token-bearing link and
        // is never logged from here (ADR 0026).
        debug!(
            to = %message.to,
            subject = %message.subject,
            "sending a message through the email provider"
        );

        let body = SendRequest {
            from: &self.from,
            to: [&message.to],
            subject: &message.subject,
            text: &message.text,
        };

        let response = self
            .http
            .post(&self.base_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                // The transport error names the URL at worst; the bearer
                // token travels in a header and never appears in it.
                error!(to = %message.to, error = %err, "the email provider could not be reached");
                EmailError::Transport(err.to_string())
            })?;

        let status = response.status();
        if !status.is_success() {
            // The response body is deliberately not read: a provider that
            // echoes the message back must not get it into the log.
            error!(
                status = status.as_u16(),
                to = %message.to,
                "the email provider refused the message"
            );
            return Err(EmailError::Provider {
                status: status.as_u16(),
            }
            .into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use tokio::net::TcpListener;
    use tracing::instrument::WithSubscriber;

    use super::*;
    use crate::email::capture::CaptureWriter;

    /// Obviously fake values throughout (rule 3).
    const API_KEY: &str = "re_not_a_real_api_key";
    const TOKEN: &str = "not-a-real-token";
    const FROM: &str = "mars@example.invalid";

    /// What the fake provider saw, and what it answers with.
    #[derive(Clone)]
    struct Fake {
        answer: StatusCode,
        seen: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
    }

    async fn record(State(fake): State<Fake>, headers: HeaderMap, body: String) -> StatusCode {
        let parsed: Value = serde_json::from_str(&body).expect("the client sends JSON");
        fake.seen
            .lock()
            .expect("the fake provider lock is healthy")
            .push((headers, parsed));
        fake.answer
    }

    /// Start a fake Resend on an ephemeral port and return its URL.
    async fn spawn_fake(answer: StatusCode) -> (String, Arc<Mutex<Vec<(HeaderMap, Value)>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake {
            answer,
            seen: Arc::clone(&seen),
        };
        let router = Router::new()
            .route("/emails", post(record))
            .with_state(fake);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port is available");
        let addr = listener.local_addr().expect("the listener has an address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        (format!("http://{addr}/emails"), seen)
    }

    fn invitation() -> (EmailMessage, String) {
        let link = format!("https://mars.example.invalid/invite/{TOKEN}");
        let expires = Utc
            .with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
            .single()
            .expect("a valid instant");
        (
            EmailMessage::invitation("someone@example.test", &link, expires),
            link,
        )
    }

    #[tokio::test]
    async fn a_successful_send_posts_the_documented_body_and_logs_no_link() {
        let (base_url, seen) = spawn_fake(StatusCode::OK).await;
        let client =
            ResendClient::new(API_KEY.to_string(), FROM.to_string()).with_base_url(base_url);
        let (message, link) = invitation();
        let writer = CaptureWriter::new();

        client
            .send(message.clone())
            .with_subscriber(writer.subscriber())
            .await
            .expect("a 2xx answer is a delivery");

        let seen = seen.lock().expect("the fake provider lock is healthy");
        let (headers, body) = seen.first().expect("the provider was called once");
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some(format!("Bearer {API_KEY}").as_str())
        );
        assert_eq!(body["from"], Value::from(FROM));
        assert_eq!(body["to"], serde_json::json!(["someone@example.test"]));
        assert_eq!(body["subject"], Value::from(message.subject.as_str()));
        assert_eq!(body["text"], Value::from(message.text.as_str()));

        let logged = writer.contents();
        assert!(!logged.contains(&link), "the link was logged: {logged}");
        assert!(!logged.contains(TOKEN), "the token was logged: {logged}");
    }

    #[tokio::test]
    async fn a_refusal_is_a_provider_error_and_logs_no_link() {
        let (base_url, _seen) = spawn_fake(StatusCode::INTERNAL_SERVER_ERROR).await;
        let client =
            ResendClient::new(API_KEY.to_string(), FROM.to_string()).with_base_url(base_url);
        let (message, link) = invitation();
        let writer = CaptureWriter::new();

        let error = client
            .send(message)
            .with_subscriber(writer.subscriber())
            .await
            .expect_err("a 500 from the provider is a failure");

        assert!(
            matches!(error, Error::Email(EmailError::Provider { status: 500 })),
            "unexpected error: {error}"
        );

        let logged = writer.contents();
        assert!(!logged.contains(&link), "the link was logged: {logged}");
        assert!(!logged.contains(TOKEN), "the token was logged: {logged}");
        assert!(!logged.contains(API_KEY), "the key was logged: {logged}");
    }

    #[tokio::test]
    async fn an_unreachable_provider_is_a_transport_error() {
        // Bound and dropped, so the port is free and nothing answers on it.
        let base_url = {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("an ephemeral port is available");
            let addr = listener.local_addr().expect("the listener has an address");
            format!("http://{addr}/emails")
        };
        let client =
            ResendClient::new(API_KEY.to_string(), FROM.to_string()).with_base_url(base_url);
        let (message, _link) = invitation();

        let error = client
            .send(message)
            .await
            .expect_err("nothing is listening");
        assert!(
            matches!(error, Error::Email(EmailError::Transport(_))),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn a_blank_recipient_never_reaches_the_provider() {
        let (base_url, seen) = spawn_fake(StatusCode::OK).await;
        let client =
            ResendClient::new(API_KEY.to_string(), FROM.to_string()).with_base_url(base_url);
        let expires = Utc
            .with_ymd_and_hms(2026, 3, 4, 5, 6, 7)
            .single()
            .expect("a valid instant");

        let error = client
            .send(EmailMessage::invitation(
                "",
                "https://example.invalid/x",
                expires,
            ))
            .await
            .expect_err("a message with no recipient is refused");

        assert!(matches!(error, Error::Email(EmailError::Configuration(_))));
        assert!(
            seen.lock()
                .expect("the fake provider lock is healthy")
                .is_empty()
        );
    }

    #[test]
    fn debug_never_prints_the_api_key() {
        let client = ResendClient::new(API_KEY.to_string(), FROM.to_string());
        let printed = format!("{client:?}");
        assert!(!printed.contains(API_KEY), "{printed}");
        assert!(printed.contains(FROM), "{printed}");
    }
}
