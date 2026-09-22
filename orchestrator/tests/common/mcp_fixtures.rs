//! The MCP conformance fixtures: what a pinned Claude Code CLI sent the Mars
//! MCP server, one HTTP request per file (`CLAUDE.md`, "Testing
//! expectations", MCP conformance fixtures).
//!
//! `tests/fixtures/mcp/<cli version>/` holds one `NN-<what>.json` per request,
//! in the order the CLI sent them, each an [`Exchange`]: the HTTP method, the
//! path, the headers that decide how the server answers, the JSON body, and
//! the status the server answered with when it was recorded. `Authorization`
//! is never among the headers (rule 3 of `CLAUDE.md`): a replay presents its
//! own seeded token.
//!
//! [`Recorder`] is the other half: an axum layer around `mcp_router` that
//! keeps every request it sees and writes them in this format. It is what
//! `scripts/mcp-record/record.sh` runs the real CLI against
//! (`images/claude/VERIFY.md`, "Recording the MCP conformance fixtures").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The headers a fixture keeps, lower-cased: the ones the transport or the
/// CLI's protocol revision gives a meaning to. `mcp-session-id` and
/// `last-event-id` are kept so that a stateful conversation shows as one: 2.1.274
/// sends neither, and the replay refuses a fixture that does until it learns to
/// substitute the session its own server handed out.
pub const RECORDED_HEADERS: [&str; 8] = [
    "host",
    "mcp-protocol-version",
    "mcp-method",
    "mcp-name",
    "mcp-session-id",
    "accept",
    "content-type",
    "last-event-id",
];

/// One request of a recorded conversation, and the status it was answered
/// with.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exchange {
    /// The HTTP method: `POST`, `GET` or `DELETE`.
    pub method: String,
    /// The request path, `/mcp`.
    pub path: String,
    /// The [`RECORDED_HEADERS`] the request carried, by lower-case name.
    pub headers: BTreeMap<String, String>,
    /// The JSON body, absent for a request without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// The status the server answered with when the fixture was recorded.
    pub status: u16,
}

impl Exchange {
    /// The JSON-RPC method of the body, if it has one.
    pub fn rpc_method(&self) -> Option<&str> {
        self.body.as_ref()?.get("method")?.as_str()
    }

    /// Whether the body is a JSON-RPC request, i.e. expects an answer.
    pub fn expects_answer(&self) -> bool {
        self.body
            .as_ref()
            .is_some_and(|body| body.get("id").is_some() && body.get("method").is_some())
    }

    /// A short name for the file: the JSON-RPC method, the tool for a call,
    /// or the HTTP method for a request without a body.
    fn slug(&self) -> String {
        let method = match self.rpc_method() {
            Some(method) => method.replace('/', "-"),
            None => return self.method.to_lowercase(),
        };
        let tool = self
            .body
            .as_ref()
            .and_then(|body| body.pointer("/params/name"))
            .and_then(Value::as_str);
        match tool {
            Some(tool) => format!("{method}-{tool}"),
            None => method,
        }
    }
}

/// The directory of one CLI version's fixtures.
pub fn version_dir(version: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp")
        .join(version)
}

/// Every recorded exchange of `dir`, in the order the CLI sent them.
pub fn load(dir: &Path) -> Vec<(String, Exchange)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("{} is readable: {err}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();

    files
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path).expect("the fixture is readable");
            let exchange = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("{name} is an exchange: {err}"));
            (name, exchange)
        })
        .collect()
}

/// Keeps every request that passes through the router it wraps.
#[derive(Clone, Default)]
pub struct Recorder {
    exchanges: Arc<Mutex<Vec<Exchange>>>,
}

impl Recorder {
    /// `router` with this recorder in front of it.
    pub fn wrap(&self, router: Router) -> Router {
        let recorder = self.clone();
        router.layer(axum::middleware::from_fn(move |request, next| {
            let recorder = recorder.clone();
            async move { recorder.record(request, next).await }
        }))
    }

    async fn record(&self, request: Request, next: Next) -> Response {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("the request body is readable");

        // Everything the request carried, by name only, so the recording run
        // shows what a fixture leaves out without printing a token.
        let names: Vec<&str> = parts.headers.keys().map(|name| name.as_str()).collect();
        println!("recorder: {} {} headers {names:?}", parts.method, parts.uri);

        let headers = RECORDED_HEADERS
            .iter()
            .filter_map(|name| {
                let value = parts.headers.get(*name)?.to_str().ok()?;
                Some(((*name).to_string(), value.to_string()))
            })
            .collect();
        let body_json = (!bytes.is_empty()).then(|| {
            serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                panic!("the CLI sent a body that is not JSON ({err}): {bytes:?}")
            })
        });

        // The slot is taken on arrival, so the files keep the order the CLI
        // sent the requests in even when two are in flight at once.
        let index = {
            let mut exchanges = self.exchanges.lock().expect("the recorder lock");
            exchanges.push(Exchange {
                method: parts.method.to_string(),
                path: parts.uri.path().to_string(),
                headers,
                body: body_json,
                status: 0,
            });
            exchanges.len() - 1
        };

        let response = next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;

        self.exchanges.lock().expect("the recorder lock")[index].status =
            response.status().as_u16();
        response
    }

    /// How many requests have been recorded so far.
    pub fn len(&self) -> usize {
        self.exchanges.lock().expect("the recorder lock").len()
    }

    /// Write one `NN-<what>.json` per recorded request into `dir`, which must
    /// not exist yet: a recorded version is never overwritten (`CLAUDE.md`,
    /// "Testing expectations").
    pub fn write(&self, dir: &Path) {
        assert!(
            !dir.exists(),
            "{} already exists; a recorded version is never overwritten",
            dir.display()
        );
        std::fs::create_dir_all(dir).expect("the fixture directory is created");

        let exchanges = self.exchanges.lock().expect("the recorder lock");
        for (index, exchange) in exchanges.iter().enumerate() {
            assert!(
                !exchange.headers.contains_key("authorization"),
                "a fixture never keeps the bearer token"
            );
            let path = dir.join(format!("{:02}-{}.json", index + 1, exchange.slug()));
            let mut text = serde_json::to_string_pretty(exchange).expect("an exchange serialises");
            text.push('\n');
            std::fs::write(&path, text).expect("the fixture is written");
        }
    }
}
