use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
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

#[tokio::test]
async fn mock_transport_connect_error_bubbles_with_connect_kind() {
    let transport = adapter_with_behavior(MockBehavior::connect_error("dns failed", None, true));
    let result = transport
        .execute(RestRequest::get("https://api.example.com/panic"))
        .await;

    let err = result.expect_err("connect mock should fail");
    assert_error_kind(err, RestErrorKind::Connect, true);
}

#[tokio::test]
async fn mock_transport_send_error_bubbles_with_send_kind() {
    let transport = adapter_with_behavior(MockBehavior::send_error("send failed", Some(0), false));
    let result = transport
        .execute(RestRequest::get("https://api.example.com/panic"))
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
        .execute(RestRequest::post("https://api.example.com/panic"))
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
        .execute(RestRequest::get("https://api.example.com/panic"))
        .await
        .expect_err("timeout mock should fail");
    assert_error_kind(timeout_err, RestErrorKind::Timeout, true);

    let internal_err = transport
        .execute(RestRequest::get("https://api.example.com/panic"))
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
        .execute(request.clone())
        .await
        .expect_err("reject behavior should be surfaced");
    assert_error_kind(execute_err, RestErrorKind::Rejected, true);

    let checked_err = transport
        .execute_checked(request)
        .await
        .expect_err("checked execution should fail on rejected responses");
    assert_error_kind(checked_err, RestErrorKind::Rejected, true);
}

#[tokio::test]
async fn mock_transport_fallback_response_is_successful_when_queue_is_empty() {
    let transport = Client::with_transport(MockRestAdapter::new());
    let response = transport
        .execute(RestRequest::get("https://api.example.com/panic"))
        .await
        .expect("mock with empty queue should return fallback response");

    assert_eq!(response.status(), 200);
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
            .get_url(url)
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
async fn post_json_uses_serialization_and_returns_mock_response() {
    let adapter = MockRestAdapter::new();
    adapter.queue_post_response(
        "https://api.example.com/echo",
        MockResponse::new(201, "created"),
    );
    let transport = Client::with_transport(adapter);

    let response = transport
        .post_json("https://api.example.com/echo", &[("value", "ok")])
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
        .get_url("https://api.example.com/zero-copy")
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
    let execute_allocation_count = take_allocs();
    assert!(execute_allocation_count > 0);

    reset_alloc_counter();
    let copied_body = payload.to_vec();
    let _copied_parse: Vec<u32> =
        sonic_rs::from_slice(&copied_body).expect("copied body parse should work");
    let copied_allocation_count = take_allocs();

    eprintln!(
        "allocation profile: direct parse={execute_allocation_count}, copied to_vec={copied_allocation_count}"
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
