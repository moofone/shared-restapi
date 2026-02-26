use bytes::Bytes;
use shared_restapi::{
    Client, MockBehavior, MockBehaviorPlan, MockResponse, MockRestAdapter, RestError,
    RestErrorKind, RestRequest,
};

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
