//! Minimal zero-copy wrapper around reqwest with an in-memory mock transport for
//! fully deterministic tests.

#![allow(dead_code)]

pub mod adapter;
pub mod fixture_policy;
pub mod mock;

pub use reqwest::Method;

pub use adapter::{
    Client, ReqwestTransport, RestBytes, RestError, RestErrorKind, RestFuture, RestRequest, RestResponse,
    RestResult, RestRetryPolicy, RestTransport, RestTransportState,
};
pub use fixture_policy::{
    RestFixtureRequirement, clear_required_rest_contracts_for_tests, ensure_live_request_allowed,
    fixture_capture_mode_enabled as rest_fixture_capture_mode_enabled,
    register_required_rest_contracts, required_rest_contracts,
};
pub use mock::{
    MockBehavior, MockBehaviorPlan, MockOperation, MockResponse, MockRestAdapter,
    MockRestStateSnapshot, MockScenario, MockScenarioStep, MockScenarioStepKind,
};
