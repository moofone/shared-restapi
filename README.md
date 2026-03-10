[![CI](https://github.com/moofone/shared-restapi/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/moofone/shared-restapi/actions/workflows/ci.yml)
[![Latest Tag](https://img.shields.io/github/v/tag/moofone/shared-restapi?sort=semver)](https://github.com/moofone/shared-restapi/tags)

Wrapper crate around `reqwest` for shared REST access with deterministic mock control in tests. Designed to minimize allocations while keeping a simple, production-friendly adapter surface.
Rate limiting is intentionally layered separately and should be composed with
`https://github.com/moofone/shared-rate_limiter` in callers that need request pacing.
`shared-restapi` provides a tiny abstraction for HTTP clients that mirrors the local adapter style used elsewhere in the workspace:

### Parse path

`RestResponse::json::<T>(&self)` parses directly from the response bytes with `sonic-rs` and
supports borrowed output types whose lifetimes are tied to the response body:

- there is no intermediate `String` conversion
- no explicit JSON AST or intermediate object-blob step
- `T` is deserialized in one pass from `&[u8]`
- borrowed fields like `&str` can be zero-copy as long as the `RestResponse` stays alive

`Client::execute_json*` remains the owned convenience path. If you need borrowed zero-copy
structs, fetch a `RestResponse` first with `get_response` / `get_checked_response`, then call
`response.json::<BorrowedType>()`.

## Mocking

The mock adapter supports deterministic behavior control for tests:

- `Pass`, `Delay`, `Reject`, `Drop`, `Replay`, and explicit mock transport errors
- queued default responses and per-route queued responses (`method + url`)
- call snapshots and counters for assertions

Use it when you need tests that assert exact transport behavior without outbound network calls.

## Boundary Fixture Rule

When `shared-restapi` is used behind an exchange or external-contract boundary, the owning
boundary actor should keep raw contract fixtures under its own `test/fixtures/` tree.

Recommended pattern:

- raw REST success/error fixtures live on the boundary actor only
- downstream business actors do not duplicate raw exchange payloads
- every registered live REST contract has:
  - a success fixture
  - an error fixture
  - a replay test that drives `MockRestAdapter`
- compliant fixtures must carry live provenance metadata:
  - `source = "live_capture"`
  - `captured_at_ms`
  - `capture_command`
  - `exchange_env`
- synthesized or malformed fixtures belong in a separate robustness bucket and do not satisfy
  live contract compliance
- `with_fixture_contract(...)` / `with_required_fixture_contract(...)` declare which contract a
  live request must satisfy; they do not replay fixture payloads
- normal mode still performs the real HTTP request, but only after fixture existence/provenance
  validation passes
- explicit fixture-capture mode bypasses the gate so capture workflows can refresh fixtures, and
  still performs the real HTTP request
- live REST execution should be blocked unless the contract is registered and its fixtures exist,
  and those fixtures are compliant live captures, except for explicit fixture-capture mode

This removes the need to remember fixture work manually: contract registration, fixture existence,
and replay coverage should be enforced by tests.

## Allocation notes

- For production transport, parsing happens from the existing response bytes in `RestResponse::json` with no intermediate `String` or DOM step.
- Borrowed zero-copy typed decoding is available only on `RestResponse`, where the body lifetime is still available.
- `execute_json*` is still useful for owned structs and `sonic_rs::Value`, but it is not the general borrowed zero-copy path because the body is consumed inside the call.

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
use sonic_rs::Value;

let transport = MockRestAdapter::new();
transport.queue_get_response(
    "https://api.example.com/v1/ping",
    MockResponse::new(200, b"{\"ok\":true,\"request_id\":\"ping-42\"}"),
);
let client = Client::with_transport(transport);

let ok: Value = client
    .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect("mocked success path");

assert_eq!(ok["ok"].as_bool(), Some(true));
assert_eq!(ok["request_id"].as_str(), Some("ping-42"));

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
use sonic_rs::Value;

let transport = MockRestAdapter::new();
transport.queue_get_response("https://api.example.com/v1/ping", MockResponse::text(503, "rate limited"));
let client = Client::with_transport(transport);
let fail = client
    .execute_json_checked::<Value>(RestRequest::get("https://api.example.com/v1/ping"))
    .await
    .expect_err("mocked rejection should be surfaced");
assert_eq!(fail.kind(), RestErrorKind::Rejected);
assert_eq!(fail.is_retryable(), true);
```

## Example - Production Default

```rust
use shared_restapi::{Client, RestRequest};
use sonic_rs::Value;

let client = Client::new();
let payload = bytes::Bytes::from_static(
    br#"{"jsonrpc":"2.0","id":1,"method":"public/get_order_book","params":{"instrument_name":"BTC-PERPETUAL"}}"#,
);

let candles: Value = client
    .execute_json_checked(
        RestRequest::post("https://www.deribit.com/api/v2/public/get_order_book")
            .with_body(payload),
    )
    .await
    .expect("production request should parse into typed payload");
assert!(candles.get("result").is_some());
```

## Example - Production With Retry

```rust
use shared_restapi::{Client, RestRequest};
use sonic_rs::Value;

let client = Client::new();
let payload = bytes::Bytes::from_static(
    br#"{"jsonrpc":"2.0","id":1,"method":"public/get_order_book","params":{"instrument_name":"BTC-PERPETUAL"}}"#,
);

let request = RestRequest::post("https://www.deribit.com/api/v2/public/get_order_book")
    .with_body(payload)
    .with_retry_on_4xx(2)
    .with_retry_on_statuses_extend([503], 2); // retry all 4xx plus 503

let candles: Value = client
    .execute_json_checked(request)
    .await
    .expect("request should retry up to 2 times on all 4xx statuses and 503");
assert!(candles.get("result").is_some());
```
