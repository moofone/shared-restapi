use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

use bytes::Bytes;
use reqwest::header::HeaderValue;
use reqwest::{Client as ReqwestClient, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::from_slice;

pub use reqwest::Method;

pub type RestBytes = Bytes;
pub type RestFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type RestResult<T> = Result<T, RestError>;

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
}

#[derive(Clone, Debug)]
pub struct RestError {
    pub kind: RestErrorKind,
    pub status: Option<u16>,
    pub message: String,
    pub retryable: bool,
}

impl RestError {
    pub fn new(
        kind: RestErrorKind,
        status: Option<u16>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            status,
            message: message.into(),
            retryable,
        }
    }

    fn from_reqwest(kind: RestErrorKind, err: reqwest::Error) -> Self {
        let status = err.status().map(|s| s.as_u16());
        let message = err.to_string();
        let retryable = err.is_timeout() || err.is_connect() || err.is_request();
        Self {
            kind,
            status,
            message,
            retryable,
        }
    }

    pub fn from_serde(err: serde_json::Error) -> Self {
        Self {
            kind: RestErrorKind::Parse,
            status: None,
            message: err.to_string(),
            retryable: false,
        }
    }
}

impl fmt::Display for RestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rest error {:?} status={:?} retryable={} {}",
            self.kind, self.status, self.retryable, self.message
        )
    }
}

impl Error for RestError {}

#[derive(Clone, Debug)]
pub struct RestRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, RestBytes)>,
    pub body: Option<RestBytes>,
    pub timeout: Option<Duration>,
}

impl RestRequest {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout: None,
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
}

#[derive(Clone, Debug)]
pub struct RestResponse {
    pub status: u16,
    pub headers: Vec<(String, RestBytes)>,
    pub body: RestBytes,
    pub elapsed: Duration,
}

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
        from_slice(&self.body).map_err(RestError::from_serde)
    }
}

pub trait RestTransport: Send + Sync {
    fn execute(&self, request: RestRequest) -> RestFuture<RestResult<RestResponse>>;
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

    pub async fn execute(&self, request: RestRequest) -> RestResult<RestResponse> {
        self.transport.execute(request).await
    }

    pub async fn execute_json<T>(&self, request: RestRequest) -> RestResult<T>
    where
        T: DeserializeOwned,
    {
        self.execute(request).await?.json::<T>()
    }

    pub async fn get(&self, request: RestRequest) -> RestResult<RestResponse> {
        self.execute(request).await
    }

    pub async fn get_url(&self, url: impl Into<String>) -> RestResult<RestResponse> {
        self.execute(RestRequest::get(url)).await
    }

    pub async fn post(
        &self,
        url: impl Into<String>,
        body: impl Into<RestBytes>,
    ) -> RestResult<RestResponse> {
        self.execute(RestRequest::post(url).with_body(body)).await
    }

    pub async fn post_json<T: Serialize>(
        &self,
        url: impl Into<String>,
        payload: &T,
    ) -> RestResult<RestResponse> {
        let body = serde_json::to_vec(payload).map_err(RestError::from_serde)?;
        self.post(url, body).await
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
    fn execute(&self, request: RestRequest) -> RestFuture<RestResult<RestResponse>> {
        let client = self.client.clone();
        Box::pin(async move {
            let start = Instant::now();
            let mut req = client.request(request.method.clone(), &request.url);

            for (key, value) in request.headers {
                let value = HeaderValue::from_bytes(value.as_ref())
                    .map_err(|err| RestError::new(RestErrorKind::Internal, None, err, false))?;
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
