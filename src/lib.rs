//! Minimal zero-copy wrapper around reqwest with an in-memory mock transport for
//! fully deterministic tests.

#![allow(dead_code)]

pub mod adapter;
pub mod fixture;
pub mod fixture_policy;
pub mod mock;
pub mod runtime;

pub use reqwest::Method;
pub use runtime::block_on_rest_future;

pub use adapter::{
    Client, ImpersonateTransport, ReqwestTransport, RestBytes, RestError, RestErrorKind,
    RestFuture, RestRequest, RestResponse, RestResult, RestRetryPolicy, RestTransport,
    RestTransportState,
};
pub use fixture::RestFixture;
pub use fixture_policy::{
    RestFixtureRequirement, clear_live_mode_for_tests, clear_required_rest_contracts_for_tests,
    enable_live_mode, ensure_live_request_allowed,
    fixture_capture_mode_enabled as rest_fixture_capture_mode_enabled,
    register_required_rest_contracts, required_rest_contracts, validate_required_rest_contracts,
};
pub use mock::{
    FixtureResponse, MockBehavior, MockBehaviorPlan, MockOperation, MockResponse, MockRestAdapter,
    MockRestStateSnapshot, MockScenario, MockScenarioStep, MockScenarioStepKind,
};
