use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use serde::Serialize;

use super::adapter::{
    Method, RestBytes, RestError, RestErrorKind, RestFuture, RestRequest, RestResponse, RestResult,
    RestTransport, SharedRestTransport, RestTransportState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockScenarioStepKind {
    Pass,
    Delay,
    Reject,
    Drop,
    Replay,
}

#[derive(Clone, Debug)]
pub struct MockScenarioStep {
    pub kind: MockScenarioStepKind,
    pub status: Option<u16>,
    pub message: Option<String>,
    pub delay: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct MockScenario(Vec<MockScenarioStep>);

impl MockScenario {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(mut self, step: MockScenarioStep) -> Self {
        self.0.push(step);
        self
    }

    pub fn pass(mut self) -> Self {
        self.0.push(MockScenarioStep {
            kind: MockScenarioStepKind::Pass,
            status: None,
            message: None,
            delay: None,
        });
        self
    }

    pub fn delay(mut self, duration: Duration) -> Self {
        self.0.push(MockScenarioStep {
            kind: MockScenarioStepKind::Delay,
            status: None,
            message: None,
            delay: Some(duration),
        });
        self
    }

    pub fn reject(mut self, status: u16, message: impl Into<String>) -> Self {
        self.0.push(MockScenarioStep {
            kind: MockScenarioStepKind::Reject,
            status: Some(status),
            message: Some(message.into()),
            delay: None,
        });
        self
    }

    pub fn drop_response(mut self) -> Self {
        self.0.push(MockScenarioStep {
            kind: MockScenarioStepKind::Drop,
            status: None,
            message: None,
            delay: None,
        });
        self
    }
}

impl Default for MockScenario {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum MockBehavior {
    Pass,
    Delay(Duration),
    Reject {
        status: u16,
        reason: String,
    },
    Drop,
    Replay(Vec<MockResponse>),
}

impl MockBehavior {
    pub fn pass() -> Self {
        Self::Pass
    }

    pub fn delay(ms: u64) -> Self {
        Self::Delay(Duration::from_millis(ms))
    }

    pub fn reject(status: u16, reason: impl Into<String>) -> Self {
        Self::Reject {
            status,
            reason: reason.into(),
        }
    }

    pub fn drop_response() -> Self {
        Self::Drop
    }

    pub fn replay(frames: impl IntoIterator<Item = MockResponse>) -> Self {
        Self::Replay(frames.into_iter().collect())
    }
}

#[derive(Clone, Debug)]
pub enum MockOperation {
    Request,
}

#[derive(Clone, Debug, Default)]
pub struct MockBehaviorPlan {
    request: VecDeque<MockBehavior>,
    scenario: VecDeque<MockScenarioStep>,
}

impl MockBehaviorPlan {
    pub fn push(&mut self, behavior: MockBehavior) -> &mut Self {
        self.request.push_back(behavior);
        self
    }

    pub fn push_request(&mut self, behavior: MockBehavior) -> &mut Self {
        self.push(behavior)
    }

    pub fn pop(&mut self, _operation: MockOperation) -> MockBehavior {
        match _operation {
            MockOperation::Request => self.request.pop_front().unwrap_or_default(),
        }
    }

    pub fn push_scenario_step(&mut self, step: MockScenarioStep) -> &mut Self {
        self.scenario.push_back(step);
        self
    }

    pub fn scenario(scenario: MockScenario) -> Self {
        let steps = scenario.0;
        Self {
            request: steps
                .iter()
                .map(|step| match step.kind {
                    MockScenarioStepKind::Pass => MockBehavior::Pass,
                    MockScenarioStepKind::Delay => {
                        MockBehavior::Delay(step.delay.unwrap_or_else(|| Duration::from_millis(0)))
                    }
                    MockScenarioStepKind::Reject => {
                        MockBehavior::Reject {
                            status: step.status.unwrap_or(500),
                            reason: step.message.unwrap_or_else(|| "rejected".to_string()),
                        }
                    }
                    MockScenarioStepKind::Drop => MockBehavior::Drop,
                    MockScenarioStepKind::Replay => MockBehavior::Pass,
                })
                .collect(),
            scenario: steps.into_iter().collect(),
        }
    }
}

impl Default for MockBehavior {
    fn default() -> Self {
        Self::Pass
    }
}

#[derive(Clone, Debug)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, RestBytes)>,
    pub body: RestBytes,
}

impl MockResponse {
    pub fn new(status: u16, body: impl Into<RestBytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<RestBytes>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn json<T: Serialize>(status: u16, payload: &T) -> RestResult<Self> {
        let body = serde_json::to_vec(payload).map_err(RestError::from_serde)?;
        Ok(Self::new(status, body))
    }
}

#[derive(Clone, Debug)]
pub struct MockRestStateSnapshot {
    pub state: RestTransportState,
    pub request_count: usize,
    pub last_url: Option<String>,
    pub last_status: Option<u16>,
    pub behavior_remaining: usize,
    pub response_queue_len: usize,
    pub route_queue_len: usize,
    pub inbound_count: usize,
    pub outbound_count: usize,
    pub elapsed_total: Duration,
    pub last_error: Option<String>,
}

#[derive(Debug)]
struct MockRestAdapterState {
    pub state: RestTransportState,
    pub request_count: usize,
    pub last_url: Option<String>,
    pub last_status: Option<u16>,
    pub behavior_plan: MockBehaviorPlan,
    pub default_response_queue: VecDeque<MockResponse>,
    pub route_response_queues: HashMap<(Method, String), VecDeque<MockResponse>>,
    pub outbound_log: Vec<RestRequest>,
    pub inbound_log: Vec<RestResponse>,
    pub last_error: Option<String>,
    pub elapsed_total: Duration,
}

impl MockRestAdapterState {
    fn snapshot(&self) -> MockRestStateSnapshot {
        MockRestStateSnapshot {
            state: self.state,
            request_count: self.request_count,
            last_url: self.last_url.clone(),
            last_status: self.last_status,
            behavior_remaining: self.behavior_plan.request.len(),
            response_queue_len: self.default_response_queue.len(),
            route_queue_len: self
                .route_response_queues
                .values()
                .map(VecDeque::len)
                .sum(),
            inbound_count: self.inbound_log.len(),
            outbound_count: self.outbound_log.len(),
            elapsed_total: self.elapsed_total,
            last_error: self.last_error.clone(),
        }
    }
}

impl Default for MockRestAdapterState {
    fn default() -> Self {
        Self {
            state: RestTransportState::Idle,
            request_count: 0,
            last_url: None,
            last_status: None,
            behavior_plan: MockBehaviorPlan::default(),
            default_response_queue: VecDeque::new(),
            route_response_queues: HashMap::new(),
            outbound_log: Vec::new(),
            inbound_log: Vec::new(),
            last_error: None,
            elapsed_total: Duration::from_millis(0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MockRestAdapter {
    state: Arc<Mutex<MockRestAdapterState>>,
}

impl MockRestAdapter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockRestAdapterState::default())),
        }
    }

    pub fn with_behavior_plan(behavior_plan: MockBehaviorPlan) -> Self {
        let mut state = MockRestAdapterState::default();
        state.behavior_plan = behavior_plan;
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn from_scenario(scenario: MockScenario) -> Self {
        Self::with_behavior_plan(MockBehaviorPlan::scenario(scenario))
    }

    pub fn snapshot(&self) -> MockRestStateSnapshot {
        self.state
            .lock()
            .expect("mock-restapi mutex poisoned while taking snapshot")
            .snapshot()
    }

    pub fn queue_response(&self, response: MockResponse) {
        self.state
            .lock()
            .expect("mock-restapi mutex poisoned while queueing response")
            .default_response_queue
            .push_back(response);
    }

    pub fn queue_response_for(&self, method: Method, url: impl Into<String>, response: MockResponse) {
        let key = (method, url.into());
        self.state
            .lock()
            .expect("mock-restapi mutex poisoned while queueing response by route")
            .route_response_queues
            .entry(key)
            .or_default()
            .push_back(response);
    }

    pub fn queue_post_response(&self, url: impl Into<String>, response: MockResponse) {
        self.queue_response_for(Method::POST, url, response);
    }

    pub fn queue_get_response(&self, url: impl Into<String>, response: MockResponse) {
        self.queue_response_for(Method::GET, url, response);
    }

    pub fn outbound_count(&self) -> usize {
        self.state
            .lock()
            .expect("mock-restapi mutex poisoned while reading outbound count")
            .outbound_log
            .len()
    }

    pub fn inbound_count(&self) -> usize {
        self.state
            .lock()
            .expect("mock-restapi mutex poisoned while reading inbound count")
            .inbound_log
            .len()
    }

    pub fn clear_logs(&self) {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while clearing logs");
        state.outbound_log.clear();
        state.inbound_log.clear();
    }

    fn pop_scenario_step(&self, operation: MockOperation) -> Option<MockScenarioStep> {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while reading scenario");
        match operation {
            MockOperation::Request => state.behavior_plan.scenario.pop_front(),
        }
    }

    fn pop_behavior(&self, operation: MockOperation) -> MockBehavior {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while reading behavior plan");
        let behavior = state.behavior_plan.pop(operation);
        if let Some(step) = self
            .pop_scenario_step(operation)
        {
            match step.kind {
                MockScenarioStepKind::Drop => return MockBehavior::Drop,
                MockScenarioStepKind::Pass => {}
                MockScenarioStepKind::Delay => {
                    if let Some(delay) = step.delay {
                        return MockBehavior::Delay(delay);
                    }
                }
                MockScenarioStepKind::Reject => {
                    return MockBehavior::Reject {
                        status: step.status.unwrap_or(500),
                        reason: step.message.unwrap_or_else(|| "rejected".to_string()),
                    };
                }
                MockScenarioStepKind::Replay => {}
            }
        }
        behavior
    }

    fn apply_delay(behavior: &MockBehavior) {
        if let MockBehavior::Delay(duration) = behavior {
            std::thread::sleep(*duration);
        }
    }

    fn next_default_response(&self, request: &RestRequest) -> Option<MockResponse> {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while selecting default response");
        let route_key = (request.method.clone(), request.url.clone());
        if let Some(queue) = state.route_response_queues.get_mut(&route_key) {
            if let Some(response) = queue.pop_front() {
                return Some(response);
            }
        }
        state.default_response_queue.pop_front()
    }

    fn push_inbound_log(&self, response: RestResponse) {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while pushing inbound log");
        state.inbound_log.push(response);
    }

    fn push_outbound_log(&self, request: RestRequest) {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while pushing outbound log");
        state.outbound_log.push(request);
    }

    fn error(&self, kind: RestErrorKind, status: Option<u16>, message: impl Into<String>, retryable: bool) {
        let mut state = self
            .state
            .lock()
            .expect("mock-restapi mutex poisoned while recording error");
        state.state = RestTransportState::Error;
        let message = message.into();
        state.last_error = Some(message.clone());
        state.last_status = status;
        RestError::new(kind, status, message, retryable)
    }
}

impl Default for MockRestAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RestTransport for MockRestAdapter {
    fn execute(&self, request: RestRequest) -> RestFuture<RestResult<RestResponse>> {
        let adapter = self.clone();
        Box::pin(async move {
            let behavior = adapter.pop_behavior(MockOperation::Request);
            Self::apply_delay(&behavior);

            let start = Instant::now();
            adapter.push_outbound_log(request.clone());

            let mut state = adapter
                .state
                .lock()
                .expect("mock-restapi mutex poisoned while updating state before execute");
            state.request_count += 1;
            state.last_url = Some(request.url.clone());
            state.state = RestTransportState::Busy;
            state.last_error = None;
            drop(state);

            match behavior {
                MockBehavior::Drop => {
                    let error = adapter.error(
                        RestErrorKind::Timeout,
                        None,
                        "mock transport dropped response",
                        false,
                    );
                    return Err(error);
                }
                MockBehavior::Reject { status, reason } => {
                    return Err(adapter.error(
                        RestErrorKind::Rejected,
                        Some(status),
                        reason,
                        true,
                    ));
                }
                MockBehavior::Delay(_) | MockBehavior::Pass | MockBehavior::Replay(_) => {}
            }

            let maybe_response = if let MockBehavior::Replay(list) = behavior {
                let mut state = adapter
                    .state
                    .lock()
                    .expect("mock-restapi mutex poisoned while enqueueing replay responses");
                state
                    .default_response_queue
                    .extend(list.into_iter());
                drop(state);
                adapter.next_default_response(&request)
            } else {
                adapter.next_default_response(&request)
            };

            let response = match maybe_response {
                Some(response) => {
                    let elapsed = start.elapsed();
                    let response = RestResponse {
                        status: response.status,
                        headers: response.headers,
                        body: response.body,
                        elapsed,
                    };
                    adapter.push_inbound_log(response.clone());
                    {
                        let mut state = adapter
                            .state
                            .lock()
                            .expect("mock-restapi mutex poisoned while recording inbound response");
                        state.last_status = Some(response.status);
                        state.state = RestTransportState::Idle;
                        state.elapsed_total += elapsed;
                    }
                    Ok(response)
                }
                None => {
                    let fallback = RestResponse {
                        status: 200,
                        headers: Vec::new(),
                        body: Bytes::new(),
                        elapsed: start.elapsed(),
                    };
                    adapter.push_inbound_log(fallback.clone());
                    {
                        let mut state = adapter
                            .state
                            .lock()
                            .expect("mock-restapi mutex poisoned while recording fallback response");
                        state.last_status = Some(200);
                        state.state = RestTransportState::Idle;
                    }
                    Ok(fallback)
                }
            };

            response
        })
    }
}
