use std::{future::Future, pin::Pin, time::Duration, time::Instant};

use bytes::Bytes;
use reqwest::header::HeaderValue;
use reqwest::{Client as ReqwestClient, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sonic_rs::from_slice;
use thiserror::Error;

pub type RestBytes = Bytes;
pub type RestFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type RestResult<T> = Result<T, RestError>;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Request state for a mock that mirrors transport behavior (optional for callers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestTransportState {
    Idle,
    Busy,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestErrorKind {
    Connect,
    Send,
    Receive,
    Timeout,
    Rejected,
    Parse,
    Internal,
    MockTransport,
}

#[derive(Error, Debug)]
pub enum RestError {
    #[error("connect transport error: {message}")]
    Connect {
        #[source]
        source: reqwest::Error,
        status: Option<u16>,
        retryable: bool,
        message: String,
    },

    #[error("request send error: {message}")]
    Send {
        #[source]
        source: reqwest::Error,
        status: Option<u16>,
        retryable: bool,
        message: String,
    },

    #[error("response receive error: {message}")]
    Receive {
        #[source]
        source: reqwest::Error,
        status: Option<u16>,
        retryable: bool,
        message: String,
    },

    #[error("request timeout: {message}")]
    Timeout {
        status: Option<u16>,
        retryable: bool,
        message: String,
    },

    #[error("request rejected (status={status}): {reason}")]
    Rejected {
        status: u16,
        reason: String,
        retryable: bool,
    },

    #[error("response parse error: {0}")]
    Parse(#[from] sonic_rs::Error),

    #[error("internal transport error: {message}")]
    Internal { message: String },

    #[error("mock transport behavior error: {message}")]
    MockTransport {
        kind: RestErrorKind,
        status: Option<u16>,
        retryable: bool,
        message: String,
    },
}

impl RestError {
    fn from_reqwest(kind: RestErrorKind, err: reqwest::Error) -> Self {
        let status = err.status().map(|status| status.as_u16());
        let message = err.to_string();
        if err.is_timeout() {
            return Self::Timeout {
                status,
                retryable: true,
                message,
            };
        }
        let retryable = match kind {
            RestErrorKind::Connect => err.is_connect(),
            RestErrorKind::Send => err.is_request() || err.is_connect(),
            RestErrorKind::Receive => err.is_request(),
            RestErrorKind::Timeout => true,
            _ => false,
        };

        match kind {
            RestErrorKind::Connect => Self::Connect {
                source: err,
                status,
                retryable,
                message,
            },
            RestErrorKind::Send => Self::Send {
                source: err,
                status,
                retryable,
                message,
            },
            _ => Self::Receive {
                source: err,
                status,
                retryable,
                message,
            },
        }
    }

    pub fn timeout(message: impl Into<String>, status: Option<u16>, retryable: bool) -> Self {
        Self::Timeout {
            status,
            retryable,
            message: message.into(),
        }
    }

    pub fn rejected(status: u16, reason: impl Into<String>, retryable: bool) -> Self {
        Self::Rejected {
            status,
            reason: reason.into(),
            retryable,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    pub fn mock(
        kind: RestErrorKind,
        message: impl Into<String>,
        status: Option<u16>,
        retryable: bool,
    ) -> Self {
        Self::MockTransport {
            kind,
            status,
            retryable,
            message: message.into(),
        }
    }

    pub fn mock_response(message: impl Into<String>, status: Option<u16>, retryable: bool) -> Self {
        Self::mock(RestErrorKind::MockTransport, message, status, retryable)
    }

    pub fn connect(message: impl Into<String>, status: Option<u16>, retryable: bool) -> Self {
        Self::mock(RestErrorKind::Connect, message, status, retryable)
    }

    pub fn send(message: impl Into<String>, status: Option<u16>, retryable: bool) -> Self {
        Self::mock(RestErrorKind::Send, message, status, retryable)
    }

    pub fn receive(message: impl Into<String>, status: Option<u16>, retryable: bool) -> Self {
        Self::mock(RestErrorKind::Receive, message, status, retryable)
    }

    pub fn parse(err: sonic_rs::Error) -> Self {
        Self::Parse(err)
    }

    pub fn kind(&self) -> RestErrorKind {
        match self {
            Self::Connect { .. } => RestErrorKind::Connect,
            Self::Send { .. } => RestErrorKind::Send,
            Self::Receive { .. } => RestErrorKind::Receive,
            Self::Timeout { .. } => RestErrorKind::Timeout,
            Self::Rejected { .. } => RestErrorKind::Rejected,
            Self::Parse(_) => RestErrorKind::Parse,
            Self::Internal { .. } => RestErrorKind::Internal,
            Self::MockTransport { kind, .. } => *kind,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Connect { status, .. } => *status,
            Self::Send { status, .. } => *status,
            Self::Receive { status, .. } => *status,
            Self::Timeout { status, .. } => *status,
            Self::Rejected { status, .. } => Some(*status),
            Self::Parse(_) => None,
            Self::Internal { .. } => None,
            Self::MockTransport { status, .. } => *status,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Connect { retryable, .. } => *retryable,
            Self::Send { retryable, .. } => *retryable,
            Self::Receive { retryable, .. } => *retryable,
            Self::Timeout { retryable, .. } => *retryable,
            Self::Rejected { retryable, .. } => *retryable,
            Self::Parse(_) => false,
            Self::Internal { .. } => false,
            Self::MockTransport { retryable, .. } => *retryable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestRetryPolicy {
    pub max_retries: usize,
    pub statuses: Vec<u16>,
}

impl RestRetryPolicy {
    fn should_retry(&self, status: u16, attempt: usize) -> bool {
        attempt < self.max_retries && self.statuses.contains(&status)
    }
}

#[derive(Clone, Debug)]
pub struct RestRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, RestBytes)>,
    pub body: Option<RestBytes>,
    pub timeout: Option<Duration>,
    pub retry_policy: Option<RestRetryPolicy>,
}

impl RestRequest {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: Some(DEFAULT_REQUEST_TIMEOUT),
            retry_policy: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::GET, url)
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::POST, url)
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<RestBytes>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: impl Into<RestBytes>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn with_retry_on_status(self, status: u16, max_retries: usize) -> Self {
        self.with_retry_on_statuses([status], max_retries)
    }

    pub fn with_retry_on_statuses<I>(mut self, statuses: I, max_retries: usize) -> Self
    where
        I: IntoIterator<Item = u16>,
    {
        let statuses = statuses.into_iter().collect::<Vec<_>>();
        self.retry_policy = Some(RestRetryPolicy {
            max_retries,
            statuses,
        });
        self
    }

    fn should_retry_status(&self, status: u16, attempt: usize) -> bool {
        self.retry_policy
            .as_ref()
            .is_some_and(|policy| policy.should_retry(status, attempt))
    }
}

#[derive(Clone, Debug)]
pub struct RestResponse {
    pub status: u16,
    pub headers: Vec<(String, RestBytes)>,
    pub body: RestBytes,
    pub elapsed: Duration,
}

pub type RestRawResponse = (u16, RestBytes, Duration);

impl RestResponse {
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn json<T: DeserializeOwned>(&self) -> RestResult<T> {
        from_slice(&self.body).map_err(RestError::from)
    }

    pub fn ensure_success(&self) -> RestResult<()> {
        if self.is_success() {
            Ok(())
        } else {
            let retryable = (500..600).contains(&self.status);
            let body_excerpt = String::from_utf8_lossy(&self.body);
            Err(RestError::rejected(
                self.status,
                format!(
                    "request rejected: status={} body={}",
                    self.status, body_excerpt
                ),
                retryable,
            ))
        }
    }
}

pub trait RestTransport: Send + Sync {
    fn execute(&self, request: RestRequest) -> RestFuture<RestResult<RestResponse>>;

    fn execute_raw(&self, request: RestRequest) -> RestFuture<RestResult<RestRawResponse>> {
        let future = self.execute(request);
        Box::pin(async move {
            let response = future.await?;
            Ok((response.status, response.body, response.elapsed))
        })
    }
}

pub type SharedRestTransport = dyn RestTransport + Send + Sync;

#[derive(Clone)]
pub struct Client {
    transport: std::sync::Arc<SharedRestTransport>,
}

impl Client {
    pub fn new() -> Self {
        Self::with_transport(ReqwestTransport::new())
    }

    pub fn with_transport<T>(transport: T) -> Self
    where
        T: RestTransport + 'static,
    {
        Self {
            transport: std::sync::Arc::new(transport),
        }
    }

    async fn execute(&self, request: RestRequest) -> RestResult<RestResponse> {
        self.transport.execute(request).await
    }

    async fn execute_checked(&self, request: RestRequest) -> RestResult<RestResponse> {
        let mut attempt = 0usize;
        loop {
            let response = self.execute(request.clone()).await?;
            if response.is_success() {
                return Ok(response);
            }
            if request.should_retry_status(response.status, attempt) {
                attempt += 1;
                continue;
            }
            response.ensure_success()?;
            return Ok(response);
        }
    }

    pub async fn execute_json<T>(&self, request: RestRequest) -> RestResult<T>
    where
        T: DeserializeOwned,
    {
        self.execute_json_direct(request).await
    }

    pub async fn execute_json_direct<T>(&self, request: RestRequest) -> RestResult<T>
    where
        T: DeserializeOwned,
    {
        let (_status, body, _elapsed) = self.transport.execute_raw(request).await?;
        from_slice(&body).map_err(RestError::from)
    }

    pub async fn execute_json_checked<T>(&self, request: RestRequest) -> RestResult<T>
    where
        T: DeserializeOwned,
    {
        self.execute_json_checked_direct(request).await
    }

    pub async fn execute_json_checked_direct<T>(&self, request: RestRequest) -> RestResult<T>
    where
        T: DeserializeOwned,
    {
        let mut attempt = 0usize;
        loop {
            let (status, body, _elapsed) = self.transport.execute_raw(request.clone()).await?;
            if (200..300).contains(&status) {
                return from_slice(&body).map_err(RestError::from);
            }
            if request.should_retry_status(status, attempt) {
                attempt += 1;
                continue;
            }
            let retryable = (500..600).contains(&status);
            let body_excerpt = String::from_utf8_lossy(&body);
            return Err(RestError::rejected(
                status,
                format!("request rejected: status={} body={}", status, body_excerpt),
                retryable,
            ));
        }
    }

    pub async fn get_response(&self, request: RestRequest) -> RestResult<RestResponse> {
        self.execute(request).await
    }

    pub async fn get_url_response(&self, url: impl Into<String>) -> RestResult<RestResponse> {
        self.execute(RestRequest::get(url)).await
    }

    pub async fn post_response(
        &self,
        url: impl Into<String>,
        body: impl Into<RestBytes>,
    ) -> RestResult<RestResponse> {
        self.execute(RestRequest::post(url).with_body(body)).await
    }

    pub async fn post_json_response<T: Serialize>(
        &self,
        url: impl Into<String>,
        payload: &T,
    ) -> RestResult<RestResponse> {
        let body = sonic_rs::to_vec(payload).map_err(RestError::from)?;
        self.post_response(url, body).await
    }

    pub async fn post_json_direct<TPayload: Serialize, TResponse>(
        &self,
        url: impl Into<String>,
        payload: &TPayload,
    ) -> RestResult<TResponse>
    where
        TResponse: DeserializeOwned,
    {
        let body = sonic_rs::to_vec(payload).map_err(RestError::from)?;
        self.execute_json_direct(RestRequest::post(url).with_body(body))
            .await
    }

    pub async fn post_json_checked_direct<TPayload: Serialize, TResponse>(
        &self,
        url: impl Into<String>,
        payload: &TPayload,
    ) -> RestResult<TResponse>
    where
        TResponse: DeserializeOwned,
    {
        let body = sonic_rs::to_vec(payload).map_err(RestError::from)?;
        self.execute_json_checked_direct(RestRequest::post(url).with_body(body))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: ReqwestClient,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self {
            client: ReqwestClient::new(),
        }
    }

    pub fn with_client(client: ReqwestClient) -> Self {
        Self { client }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl RestTransport for ReqwestTransport {
    fn execute_raw(&self, request: RestRequest) -> RestFuture<RestResult<RestRawResponse>> {
        let client = self.client.clone();
        Box::pin(async move {
            let start = Instant::now();
            let mut req = client.request(request.method.clone(), &request.url);

            for (key, value) in request.headers {
                let value = HeaderValue::from_bytes(value.as_ref())
                    .map_err(|err| RestError::internal(err.to_string()))?;
                req = req.header(key, value);
            }

            if let Some(body) = request.body {
                req = req.body(body);
            }

            if let Some(timeout) = request.timeout {
                req = req.timeout(timeout);
            }

            let resp = req
                .send()
                .await
                .map_err(|err| RestError::from_reqwest(RestErrorKind::Send, err))?;

            let status = resp.status().as_u16();
            let body = resp
                .bytes()
                .await
                .map_err(|err| RestError::from_reqwest(RestErrorKind::Receive, err))?;
            let elapsed = start.elapsed();

            Ok((status, body, elapsed))
        })
    }

    fn execute(&self, request: RestRequest) -> RestFuture<RestResult<RestResponse>> {
        let client = self.client.clone();
        Box::pin(async move {
            let start = Instant::now();
            let mut req = client.request(request.method.clone(), &request.url);

            for (key, value) in request.headers {
                let value = HeaderValue::from_bytes(value.as_ref())
                    .map_err(|err| RestError::internal(err.to_string()))?;
                req = req.header(key, value);
            }

            if let Some(body) = request.body {
                req = req.body(body);
            }

            if let Some(timeout) = request.timeout {
                req = req.timeout(timeout);
            }

            let resp = req
                .send()
                .await
                .map_err(|err| RestError::from_reqwest(RestErrorKind::Send, err))?;

            let status = resp.status().as_u16();
            let headers = resp
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), Bytes::copy_from_slice(value.as_ref())))
                .collect();
            let body = resp
                .bytes()
                .await
                .map_err(|err| RestError::from_reqwest(RestErrorKind::Receive, err))?;
            let elapsed = start.elapsed();

            Ok(RestResponse {
                status,
                headers,
                body,
                elapsed,
            })
        })
    }
}
