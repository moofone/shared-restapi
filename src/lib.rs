//! Minimal zero-copy wrapper around reqwest with an in-memory mock transport for
//! fully deterministic tests.

#![allow(dead_code)]

pub mod adapter;
pub mod mock;

pub use adapter::{
    Client, Method, ReqwestTransport, RestBytes, RestError, RestErrorKind, RestFuture,
    RestRequest, RestResponse, RestResult, RestTransport, RestTransportState,
};
pub use mock::{
    MockBehavior, MockBehaviorPlan, MockOperation, MockRestAdapter, MockRestStateSnapshot, MockResponse,
    MockScenario, MockScenarioStep, MockScenarioStepKind,
};
