//! Minimal zero-copy wrapper around reqwest with an in-memory mock transport for
//! fully deterministic tests.

#![allow(dead_code)]

pub mod adapter;
pub mod mock;

pub use reqwest::Method;

pub use adapter::{
    Client, ReqwestTransport, RestBytes, RestError, RestErrorKind, RestFuture, RestRequest, RestResponse,
    RestResult, RestTransport, RestTransportState,
};
pub use mock::{
    MockBehavior, MockBehaviorPlan, MockOperation, MockResponse, MockRestAdapter,
    MockRestStateSnapshot, MockScenario, MockScenarioStep, MockScenarioStepKind,
};
