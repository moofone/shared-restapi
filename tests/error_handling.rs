use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use sonic_rs::Value;
use shared_restapi::{
    Client, MockBehavior, MockBehaviorPlan, MockResponse, MockRestAdapter, RestError,
    RestErrorKind, RestRequest, RestResponse, RestResult,
};
use shared_restapi::adapter::RestTransport;
use serde::Deserialize;

#[global_allocator]
static GLOBAL: SharedRestapiTestAlloc = SharedRestapiTestAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

struct SharedRestapiTestAlloc;

unsafe impl GlobalAlloc for SharedRestapiTestAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.dealloc(ptr, layout);
    }
}

fn reset_alloc_counter() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
}

fn take_allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

fn adapter_with_behavior(behavior: MockBehavior) -> Client {
    let mut behavior_plan = MockBehaviorPlan::default();
    behavior_plan.push(behavior);

    Client::with_transport(MockRestAdapter::with_behavior_plan(behavior_plan))
}

fn assert_error_kind(err: RestError, expected: RestErrorKind, expected_retryable: bool) {
    assert_eq!(err.kind(), expected);
    assert_eq!(err.is_retryable(), expected_retryable);
}

#[test]
fn request_timeout_defaults_to_two_seconds_and_is_overridable() {
    let default_request = RestRequest::get("https://api.example.com/default-timeout");
    assert_eq!(default_request.timeout, Some(std::time::Duration::from_secs(2)));

    let overridden = default_request.with_timeout(std::time::Duration::from_millis(250));
    assert_eq!(
        overridden.timeout,
        Some(std::time::Duration::from_millis(250))
    );
}

#[test]
fn request_can_set_timeout_and_retry_policy_together() {
    let request = RestRequest::get("https://api.example.com/timeout-retry")
        .with_timeout(std::time::Duration::from_millis(1500))
        .with_retry_on_status(503, 2);

    assert_eq!(
        request.timeout,
        Some(std::time::Duration::from_millis(1500))
    );
    let policy = request
        .retry_policy
        .expect("retry policy should be configured");
    assert_eq!(policy.max_retries, 2);
    assert_eq!(policy.statuses, vec![503]);
}

#[test]
fn retry_helpers_build_expected_retry_policy() {
    let request = RestRequest::get("https://api.example.com/retry")
        .with_retry_on_4xx(2)
        .with_retry_on_statuses_extend([503], 2);

    let policy = request
        .retry_policy
        .expect("retry policy should be configured");
    assert_eq!(policy.max_retries, 2);
    assert!(policy.statuses.contains(&400));
    assert!(policy.statuses.contains(&499));
    assert!(policy.statuses.contains(&503));
}

#[tokio::test]
async fn timeout_error_is_not_retried_even_when_status_retry_policy_is_set() {
    let mut behavior_plan = MockBehaviorPlan::default();
    behavior_plan.push(MockBehavior::timeout_error("timed out", Some(504), true));
    behavior_plan.push(MockBehavior::Pass);
    let adapter = MockRestAdapter::with_behavior_plan(behavior_plan);
    let transport = Client::with_transport(adapter.clone());

    let err = transport
        .execute_json_checked::<Value>(
            RestRequest::get("https://api.example.com/timeout-retry")
                .with_timeout(std::time::Duration::from_millis(50))
                .with_retry_on_status(504, 2),
        )
        .await
        .expect_err("transport timeout should fail immediately");
    assert_error_kind(err, RestErrorKind::Timeout, true);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 1);
}

#[test]
fn retry_helper_any_non_2xx_excludes_2xx() {
    let request = RestRequest::get("https://api.example.com/retry").with_retry_on_any_non_2xx(1);
    let policy = request
        .retry_policy
        .expect("retry policy should be configured");

    assert!(policy.statuses.contains(&101));
    assert!(policy.statuses.contains(&301));
    assert!(policy.statuses.contains(&503));
    assert!(!policy.statuses.contains(&200));
    assert!(!policy.statuses.contains(&250));
}

#[tokio::test]
async fn execute_json_checked_retries_configured_status_then_succeeds() {
    #[derive(Debug, Deserialize)]
    struct RetryOk {
        ok: bool,
    }

    let url = "https://api.example.com/retry-503-then-ok";
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(200, r#"{"ok":true}"#));

    let transport = Client::with_transport(adapter.clone());
    let response = transport
        .execute_json_checked::<RetryOk>(RestRequest::get(url).with_retry_on_status(503, 2))
        .await
        .expect("request should succeed after retries on configured status");
    assert!(response.ok);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 3);
}

#[tokio::test]
async fn execute_json_checked_does_not_retry_without_policy() {
    let url = "https://api.example.com/no-retry-default";
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(200, r#"{"ok":true}"#));

    let transport = Client::with_transport(adapter.clone());
    let err = transport
        .execute_json_checked::<Value>(RestRequest::get(url))
        .await
        .expect_err("request should fail immediately without retry policy");
    assert_error_kind(err, RestErrorKind::Rejected, true);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 1);
}

#[tokio::test]
async fn execute_json_checked_retries_only_on_configured_statuses() {
    let url = "https://api.example.com/retry-only-specific-status";
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(url, MockResponse::text(500, "internal"));
    adapter.queue_get_response(url, MockResponse::text(200, r#"{"ok":true}"#));

    let transport = Client::with_transport(adapter.clone());
    let err = transport
        .execute_json_checked::<Value>(RestRequest::get(url).with_retry_on_status(503, 2))
        .await
        .expect_err("status not in retry set should fail without retrying");
    assert_error_kind(err, RestErrorKind::Rejected, true);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 1);
}

#[tokio::test]
async fn execute_json_checked_stops_after_max_retries() {
    let url = "https://api.example.com/retry-exhausted";
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));

    let transport = Client::with_transport(adapter.clone());
    let err = transport
        .execute_json_checked::<Value>(RestRequest::get(url).with_retry_on_status(503, 2))
        .await
        .expect_err("request should fail after max retries are exhausted");
    assert_error_kind(err, RestErrorKind::Rejected, true);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 3);
}

#[tokio::test]
async fn execute_json_checked_with_empty_retry_statuses_does_not_retry() {
    let url = "https://api.example.com/retry-empty-statuses";
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(url, MockResponse::text(503, "temporarily unavailable"));
    adapter.queue_get_response(url, MockResponse::text(200, r#"{"ok":true}"#));

    let transport = Client::with_transport(adapter.clone());
    let err = transport
        .execute_json_checked::<Value>(RestRequest::get(url).with_retry_on_statuses([], 2))
        .await
        .expect_err("empty retry status list should behave as no retries");
    assert_error_kind(err, RestErrorKind::Rejected, true);

    let snapshot = adapter.snapshot();
    assert_eq!(snapshot.request_count, 1);
}

#[tokio::test]
async fn mock_transport_connect_error_bubbles_with_connect_kind() {
    let transport = adapter_with_behavior(MockBehavior::connect_error("dns failed", None, true));
    let result = transport
        .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/panic"))
        .await;

    let err = result.expect_err("connect mock should fail");
    assert_error_kind(err, RestErrorKind::Connect, true);
}

#[tokio::test]
async fn mock_transport_send_error_bubbles_with_send_kind() {
    let transport = adapter_with_behavior(MockBehavior::send_error("send failed", Some(0), false));
    let result = transport
        .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/panic"))
        .await;

    let err = result.expect_err("send mock should fail");
    assert_error_kind(err, RestErrorKind::Send, false);
}

#[tokio::test]
async fn mock_transport_receive_error_bubbles_with_receive_kind() {
    let transport = adapter_with_behavior(MockBehavior::receive_error(
        "connection reset",
        Some(0),
        false,
    ));
    let result = transport
        .execute_json_checked::<Value>(RestRequest::post("https://api.example.com/panic"))
        .await;

    let err = result.expect_err("receive mock should fail");
    assert_error_kind(err, RestErrorKind::Receive, false);
}

#[tokio::test]
async fn mock_transport_timeout_and_internal_errors_are_typed() {
    let mut behavior_plan = MockBehaviorPlan::default();
    behavior_plan.push(MockBehavior::timeout_error("timed out", Some(408), true));
    behavior_plan.push(MockBehavior::internal_error("state corrupted"));

    let transport = Client::with_transport(MockRestAdapter::with_behavior_plan(behavior_plan));

    let timeout_err = transport
        .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/panic"))
        .await
        .expect_err("timeout mock should fail");
    assert_error_kind(timeout_err, RestErrorKind::Timeout, true);

    let internal_err = transport
        .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/panic"))
        .await
        .expect_err("internal mock should fail");
    assert_error_kind(internal_err, RestErrorKind::Internal, false);
}

#[tokio::test]
async fn mock_transport_reject_error_maps_to_rejected_kind_and_checked_retries() {
    let mut behavior_plan = MockBehaviorPlan::default();
    behavior_plan
        .push(MockBehavior::reject(503, "rate limited"))
        .push(MockBehavior::reject(503, "rate limited"));
    let transport = Client::with_transport(MockRestAdapter::with_behavior_plan(behavior_plan));

    let request = RestRequest::get("https://api.example.com/panic");
    let execute_err = transport
        .execute_json_checked::<Value>(request.clone())
        .await
        .expect_err("reject behavior should be surfaced");
    assert_error_kind(execute_err, RestErrorKind::Rejected, true);

    let checked_err = transport
        .execute_json_checked::<Value>(request)
        .await
        .expect_err("checked execution should fail on rejected responses");
    assert_error_kind(checked_err, RestErrorKind::Rejected, true);
}

#[tokio::test]
async fn mock_transport_fallback_response_is_successful_when_queue_is_empty() {
    let transport = Client::with_transport(MockRestAdapter::new());
    let parse_error = transport
        .execute_json::<Value>(RestRequest::get("https://api.example.com/panic"))
        .await
        .expect_err("empty fallback body should fail typed json parse");
    assert_error_kind(parse_error, RestErrorKind::Parse, false);

    let response = transport
        .get_url_response("https://api.example.com/panic")
        .await
        .expect("mock with empty queue should return fallback response");
    assert!(response.body().is_empty());
}

#[tokio::test]
async fn queue_error_payload_helpers_are_supported() {
    let adapter = MockRestAdapter::new();
    adapter.queue_error_response_for(
        reqwest::Method::GET,
        "https://api.example.com/errors",
        429,
        "rate limit hit",
    );
    adapter.queue_error_text("https://api.example.com/text-error", 400, "invalid body");
    adapter
        .queue_error_json(
            "https://api.example.com/json-error",
            418,
            &std::collections::BTreeMap::from([("error", "limit")]),
        )
        .expect("json fixture should serialize for mock error response");

    let transport = Client::with_transport(adapter);

    let errors = [
        ("https://api.example.com/errors", 429),
        ("https://api.example.com/text-error", 400),
        ("https://api.example.com/json-error", 418),
    ];

    for (url, expected_status) in errors {
        let response = transport
            .get_url_response(url)
            .await
            .expect("mock queue should return configured error response");
        assert_eq!(response.status(), expected_status);
        assert!(!response.body().is_empty());
    }
}

#[tokio::test]
async fn parse_error_is_exposed_as_parse_error_kind() {
    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(
        "https://api.example.com/bad",
        MockResponse::text(200, "not-json"),
    );
    let transport = Client::with_transport(adapter);

    let parse_error = transport
        .execute_json::<String>(RestRequest::get("https://api.example.com/bad"))
        .await
        .expect_err("parse should fail for non-json body");

    assert_error_kind(parse_error, RestErrorKind::Parse, false);
}

#[tokio::test]
async fn post_response_with_bytes_returns_mock_response() {
    let adapter = MockRestAdapter::new();
    adapter.queue_post_response(
        "https://api.example.com/echo",
        MockResponse::new(201, "created"),
    );
    let transport = Client::with_transport(adapter);

    let response = transport
        .post_response(
            "https://api.example.com/echo",
            Bytes::from_static(br#"{"value":"ok"}"#),
        )
        .await
        .expect("mock response should be returned");

    assert_eq!(response.status(), 201);
}

#[tokio::test]
async fn mocked_response_body_is_zero_copy() {
    let original = Bytes::from_static(b"{\"ok\":true}");
    let original_ptr = original.as_ptr();

    let adapter = MockRestAdapter::new();
    adapter.queue_get_response(
        "https://api.example.com/zero-copy",
        MockResponse::new(200, original),
    );
    let transport = Client::with_transport(adapter);

    let response = transport
        .get_url_response("https://api.example.com/zero-copy")
        .await
        .expect("mock response should be returned");

    assert_eq!(response.body().as_ptr(), original_ptr);
}

#[tokio::test]
async fn allocation_profile_is_measurable_for_execute_json_checked() {
    #[derive(Debug, Deserialize)]
    struct AllocPayload {
        ok: bool,
        n: Vec<u32>,
    }

    let transport = HeaderHeavyTransport::new(Bytes::from_static(
        b"{\"ok\":true,\"n\":[1,2,3,4,5,6,7,8,9,10]}",
    ));
    let client = Client::with_transport(transport.clone());

    reset_alloc_counter();
    let parsed = client
        .execute_json::<AllocPayload>(RestRequest::post("https://api.example.com/alloc").with_body(
            Bytes::from_static(b"{\"ok\":true,\"n\":[1,2,3,4,5,6,7,8,9,10]}"),
        ))
        .await
        .expect("baseline parse should succeed");
    assert!(parsed.ok);
    assert_eq!(parsed.n.len(), 10);
    let execute_allocation_count = take_allocs();

    reset_alloc_counter();
    let parsed_direct = client
        .execute_json_direct::<AllocPayload>(RestRequest::post("https://api.example.com/alloc").with_body(
            Bytes::from_static(b"{\"ok\":true,\"n\":[1,2,3,4,5,6,7,8,9,10]}"),
        ))
        .await
        .expect("direct parse should succeed");
    assert!(parsed_direct.ok);
    assert_eq!(parsed_direct.n.len(), 10);
    let direct_allocation_count = take_allocs();

    eprintln!(
        "allocation profile: execute_json={execute_allocation_count}, execute_json_direct={direct_allocation_count}"
    );

    // direct path should avoid constructing mock response headers payload in the transport.
    assert!(
        direct_allocation_count <= execute_allocation_count,
        "direct path should be as allocation-light as or lighter than full response path: baseline {execute_allocation_count}, direct {direct_allocation_count}"
    );

    reset_alloc_counter();
    let payload = b"[1,2,3,4,5,6,7,8,9,10]";
    let response = RestResponse {
        status: 200,
        headers: Vec::new(),
        body: Bytes::from_static(payload),
        elapsed: std::time::Duration::from_millis(0),
    };
    let parsed = response
        .json::<Vec<u32>>()
        .expect("json parse should succeed");
    assert_eq!(parsed, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    let direct_slice_allocation_count = take_allocs();
    assert!(direct_slice_allocation_count > 0);

    reset_alloc_counter();
    let parsed_by_slice: Vec<u32> = sonic_rs::from_slice(payload).expect("parse from byte slice should work");
    assert_eq!(parsed_by_slice, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    let from_slice_allocation_count = take_allocs();

    eprintln!(
        "allocation profile: direct response json parse={direct_slice_allocation_count}, parse from borrowed bytes={from_slice_allocation_count}"
    );
    assert!(
        from_slice_allocation_count <= execute_allocation_count,
        "borrowed byte-slice parse should remain far below full execute_json allocation profile: execute_json {execute_allocation_count}, slice {from_slice_allocation_count}"
    );
}

#[derive(Clone)]
struct HeaderHeavyTransport {
    body: Bytes,
}

impl HeaderHeavyTransport {
    fn new(body: Bytes) -> Self {
        Self { body }
    }
}

impl RestTransport for HeaderHeavyTransport {
    fn execute(&self, request: RestRequest) -> shared_restapi::adapter::RestFuture<RestResult<RestResponse>> {
        let body = self.body.clone();
        let headers = (0..64)
            .map(|index| (format!("x-{index}"), Bytes::from_static(b"v")))
            .collect();
        Box::pin(async move {
            let _ = request;
            Ok(RestResponse {
                status: 200,
                headers,
                body,
                elapsed: std::time::Duration::from_millis(0),
            })
        })
    }

    fn execute_raw(&self, _request: RestRequest) -> shared_restapi::adapter::RestFuture<RestResult<(u16, Bytes, std::time::Duration)>> {
        let body = self.body.clone();
        Box::pin(async move { Ok((200, body, std::time::Duration::from_millis(0))) })
    }
}
