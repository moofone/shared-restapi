use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use shared_restapi::{Client, RestErrorKind, RestRequest};
use tokio::net::TcpListener;
use tokio::time::sleep;

#[derive(Debug, serde::Deserialize)]
struct RpcResponse<T> {
    jsonrpc: String,
    id: u64,
    result: T,
}

#[derive(Debug, serde::Deserialize)]
struct OkResult {
    ok: bool,
}

#[derive(Clone, Default)]
struct AppState {
    retry_counter: Arc<AtomicUsize>,
}

#[tokio::test]
async fn e2e_jsonrpc_success_roundtrip() {
    let server = TestServer::start().await;
    let client = Client::new();
    let payload = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"ok","params":{}}"#);

    let response: RpcResponse<OkResult> = client
        .execute_json_checked(RestRequest::post(server.url("/jsonrpc/ok")).with_body(payload))
        .await
        .expect("jsonrpc response should parse");

    assert_eq!(response.jsonrpc, "2.0");
    assert_eq!(response.id, 1);
    assert!(response.result.ok);
}

#[tokio::test]
async fn e2e_retry_on_status_then_success() {
    let server = TestServer::start().await;
    let client = Client::new();
    let payload = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"retry","params":{}}"#);

    let response: RpcResponse<OkResult> = client
        .execute_json_checked(
            RestRequest::post(server.url("/jsonrpc/retry-once"))
                .with_body(payload)
                .with_retry_on_status(503, 1),
        )
        .await
        .expect("first 503 should retry and then succeed");

    assert_eq!(response.id, 1);
    assert!(response.result.ok);
}

#[tokio::test]
async fn e2e_default_timeout_is_two_seconds() {
    let server = TestServer::start().await;
    let client = Client::new();
    let payload = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"slow","params":{}}"#);

    let err = client
        .execute_json_checked::<RpcResponse<OkResult>>(
            RestRequest::post(server.url("/jsonrpc/timeout")).with_body(payload),
        )
        .await
        .expect_err("default 2s timeout should trigger");

    assert_eq!(err.kind(), RestErrorKind::Timeout);
}

#[tokio::test]
async fn e2e_explicit_timeout_override_triggers_earlier() {
    let server = TestServer::start().await;
    let client = Client::new();
    let payload = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"slow","params":{}}"#);

    let err = client
        .execute_json_checked::<RpcResponse<OkResult>>(
            RestRequest::post(server.url("/jsonrpc/timeout"))
                .with_body(payload)
                .with_timeout(Duration::from_millis(200)),
        )
        .await
        .expect_err("explicit timeout should trigger");

    assert_eq!(err.kind(), RestErrorKind::Timeout);
}

struct TestServer {
    base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        let state = AppState::default();
        let app = Router::new()
            .route("/jsonrpc/ok", post(ok_handler))
            .route("/jsonrpc/retry-once", post(retry_once_handler))
            .route("/jsonrpc/timeout", post(timeout_handler))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{}", addr);

        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self { base_url, task }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn ok_handler() -> (StatusCode, &'static str) {
    (
        StatusCode::OK,
        r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
    )
}

async fn retry_once_handler(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if state.retry_counter.fetch_add(1, Ordering::SeqCst) == 0 {
        (StatusCode::SERVICE_UNAVAILABLE, "service unavailable")
    } else {
        (
            StatusCode::OK,
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
        )
    }
}

async fn timeout_handler() -> (StatusCode, &'static str) {
    sleep(Duration::from_millis(2500)).await;
    (
        StatusCode::OK,
        r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
    )
}
