# shared-restapi

Wrapper crate around `reqwest` for shared REST access with deterministic mock control in tests. Designed to minimize allocations while keeping a simple, production-friendly adapter surface.
Rate limiting is intentionally layered separately and should be composed with
`https://github.com/moofone/shared-rate_limiter` in callers that need request pacing.
`shared-restapi` provides a tiny abstraction for HTTP clients that mirrors the local adapter style used elsewhere in the workspace:

- a concrete `ReqwestTransport` for production
- a `RestTransport` trait for transport abstraction
- a simple `Client` facade for request execution
- a deterministic in-memory `MockRestAdapter` for fully controlled tests

### Parse path

`RestResponse::json::<T>(&self)` parses directly from the response bytes with `sonic-rs`:

- there is no intermediate `String` conversion
- no explicit JSON AST or intermediate object-blob step
- `T` is deserialized in one pass from `&[u8]`

For JSON-RPC or typed API responses, this means you deserialize directly into your Rust result type in one shot:

```rust
#[derive(serde::Deserialize)]
struct RpcEnvelope<T> {
    jsonrpc: String,
    id: u64,
    result: T,
}

let envelope: RpcEnvelope<MyPayload> = client
    .execute_json_checked(RestRequest::post("https://api.example.com/rpc").with_body(payload))
    .await?;
```

You do not need to keep a shared scratch buffer at the request level for JSON parsing. The parser reads the request/response byte buffer in place; per-response ownership of bytes is enough.

`execute_json` and `execute_json_checked` now use the direct transport byte path, so most typed JSON callers get minimum-allocation behavior by default. Use explicit raw-response methods only when you need response metadata:

- `execute_json_direct`
- `execute_json_checked_direct`
- `post_json_direct`
- `post_json_checked_direct`

These methods parse directly from transport bytes and bypass response-header materialization on the fast path where possible.

## Mocking

The mock adapter supports deterministic behavior control for tests:

- `Pass`, `Delay`, `Reject`, `Drop`, `Replay`, and explicit mock transport errors
- queued default responses and per-route queued responses (`method + url`)
- call snapshots and counters for assertions

Use it when you need tests that assert exact transport behavior without outbound network calls.

### Mocking examples

Mock response (success and error payloads):

```rust
transport.queue_response(
    MockResponse::text(200, r#"{"ok":true}"#),
);

    transport.queue_get_response(
    "https://api.example.com/v1/ping",
    MockResponse::text_error(500, "internal backend error"),
);
```

## Allocation notes

- For production transport, this crate keeps parsing zero-copy by design: parsing happens from the existing response bytes in `RestResponse::json` (no intermediate `String`/AST step).
`execute_json` now defaults to the raw-response path.

Example (normal usage):

```rust
#[derive(serde::Deserialize)]
struct Candle {
    close: f64,
}

#[derive(serde::Deserialize)]
struct DeribitCandles {
    result: Vec<Candle>,
}

let candles: DeribitCandles = client
    .post_json(
        "https://www.deribit.com/api/v2/private/get_last_trades_by_currency",
        &sonic_rs::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "public/get_order_book",
            "params": { "instrument_name": "BTC-PERPETUAL" },
        }),
    )
    .await?;
```

A measurable allocation test (`allocation_profile_is_measurable_for_execute_json_checked`) prints the delta directly in test output:

```text
allocation profile: execute_json=139, execute_json_direct=9
allocation profile: direct response json parse=7, parse from borrowed bytes=7
```

The benchmark also includes a header-heavy mock transport, so the `execute_json_direct` path shows the benefit from skipping response-header materialization.
  
In test runs, the direct path stays flat or lower than the full path by skipping header payload assembly in `execute_raw`.

Mock transport failures with typed variants:

```rust
let mut behavior_plan = MockBehaviorPlan::default();
behavior_plan
    .push(MockBehavior::connect_error("dns failure", None, true))
    .push(MockBehavior::timeout_error("upstream timeout", Some(504), true))
    .push(MockBehavior::reject(503, "rate limited"));

let transport = MockRestAdapter::with_behavior_plan(behavior_plan);
```

Error response helpers:

```rust
let _ = MockResponse::json_error(
    429,
    &sonic_rs::json!({"error":"rate_limited","message":"retry later"}),
);
let _ = MockResponse::text(400, "invalid request body");
```

## Basic usage

```rust
use shared_restapi::{
    Client,
    MockBehavior,
    MockBehaviorPlan,
    MockResponse,
    MockRestAdapter,
    RestRequest,
    RestErrorKind,
};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct PingResponse {
    ok: bool,
    request_id: String,
}

let transport = MockRestAdapter::new();
transport.queue_get_response(
    "https://api.example.com/v1/ping",
    MockResponse::new(200, b"{\"ok\":true,\"request_id\":\"ping-42\"}"),
);
let client = Client::with_transport(transport);

let ok: PingResponse = client
    .execute_json_checked::<PingResponse>(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect("mocked success path");

assert_eq!(
    ok,
    PingResponse {
        ok: true,
        request_id: "ping-42".to_string()
    }
);

// Failure path: controlled rejected mock response
let transport = MockRestAdapter::new();
transport.queue_get_response(
    "https://api.example.com/v1/ping",
    MockResponse::text(503, "rate limited"),
);
let client = Client::with_transport(transport);
let fail = client
    .execute_json_checked::<PingResponse>(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect_err("mocked rejection should be surfaced");
assert_eq!(fail.kind(), RestErrorKind::Rejected);
assert_eq!(fail.is_retryable(), false);

// Failure path: mocked transport-level error from behavior plan
let mut behavior_plan = MockBehaviorPlan::default();
behavior_plan.push(MockBehavior::connect_error("dns failure", None, true));
let transport = MockRestAdapter::with_behavior_plan(behavior_plan);
let client = Client::with_transport(transport);
let fail = client
    .execute(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect_err("connect error should be surfaced");
assert_eq!(fail.kind(), RestErrorKind::Connect);
assert_eq!(fail.is_retryable(), true);
```

## Production usage

```rust
use shared_restapi::{Client, RestRequest};
use reqwest::Method;

let client = Client::new();
let response = client
    .execute(RestRequest::new(Method::GET, "https://api.example.com/v1/data"))
    .await
    .unwrap();
```
