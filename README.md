[![CI](https://github.com/moofone/shared-restapi/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/moofone/shared-restapi/actions/workflows/ci.yml)
[![Latest Tag](https://img.shields.io/github/v/tag/moofone/shared-restapi?sort=semver)](https://github.com/moofone/shared-restapi/tags)

Wrapper crate around `reqwest` for shared REST access with deterministic mock control in tests. Designed to minimize allocations while keeping a simple, production-friendly adapter surface.
Rate limiting is intentionally layered separately and should be composed with
`https://github.com/moofone/shared-rate_limiter` in callers that need request pacing.
`shared-restapi` provides a tiny abstraction for HTTP clients that mirrors the local adapter style used elsewhere in the workspace:

### Parse path

`RestResponse::json::<T>(&self)` parses directly from the response bytes with `sonic-rs`:

- there is no intermediate `String` conversion
- no explicit JSON AST or intermediate object-blob step
- `T` is deserialized in one pass from `&[u8]`

## Mocking

The mock adapter supports deterministic behavior control for tests:

- `Pass`, `Delay`, `Reject`, `Drop`, `Replay`, and explicit mock transport errors
- queued default responses and per-route queued responses (`method + url`)
- call snapshots and counters for assertions

Use it when you need tests that assert exact transport behavior without outbound network calls.

## Allocation notes

- For production transport, this crate keeps parsing zero-copy by design: parsing happens from the existing response bytes in `RestResponse::json` (no intermediate `String`/AST step).
`execute_json` now defaults to the raw-response path.

A measurable allocation test (`allocation_profile_is_measurable_for_execute_json_checked`) prints the delta directly in test output (sample):

```text
allocation profile: execute_json=139, execute_json_direct=9
allocation profile: direct response json parse=7, parse from borrowed bytes=7
```

## Example - Mock Success

```rust
use shared_restapi::{
    Client,
    MockResponse,
    MockRestAdapter,
    RestRequest,
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

```

## Example - Mock Fail

```rust
use shared_restapi::{
    Client,
    MockRestAdapter,
    RestErrorKind,
    RestRequest,
    MockResponse,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PingResponse {
    ok: bool,
    request_id: String,
}

let transport = MockRestAdapter::new();
transport.queue_get_response("https://api.example.com/v1/ping", MockResponse::text(503, "rate limited"));
let client = Client::with_transport(transport);
let fail = client
    .execute_json_checked::<PingResponse>(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect_err("mocked rejection should be surfaced");
assert_eq!(fail.kind(), RestErrorKind::Rejected);
assert_eq!(fail.is_retryable(), true);
```

## Example - Production Default

```rust
use shared_restapi::{Client, RestRequest};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Candle {
    close: f64,
}

#[derive(Debug, Deserialize)]
struct PriceResponse {
    result: Vec<Candle>,
}

let client = Client::new();
let payload = bytes::Bytes::from_static(
    br#"{"jsonrpc":"2.0","id":1,"method":"public/get_order_book","params":{"instrument_name":"BTC-PERPETUAL"}}"#,
);

let candles: PriceResponse = client
    .execute_json_checked(
        RestRequest::post("https://www.deribit.com/api/v2/public/get_order_book")
            .with_body(payload),
    )
    .await
    .expect("production request should parse into typed payload");
```

## Example - Production With Retry

```rust
use shared_restapi::{Client, RestRequest};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PriceResponse {
    result: sonic_rs::Value,
}

let client = Client::new();
let payload = bytes::Bytes::from_static(
    br#"{"jsonrpc":"2.0","id":1,"method":"public/get_order_book","params":{"instrument_name":"BTC-PERPETUAL"}}"#,
);

let request = RestRequest::post("https://www.deribit.com/api/v2/public/get_order_book")
    .with_body(payload)
    .with_retry_on_4xx(2)
    .with_retry_on_statuses_extend([503], 2); // retry all 4xx plus 503

let candles: PriceResponse = client
    .execute_json_checked(request)
    .await
    .expect("request should retry up to 2 times on all 4xx statuses and 503");
```
