//! The HTTP layer [`download`](super::download) talks to, injectable like the `opener`
//! parameter of the Python version: tests pass a closure, the application [`UreqClient`].

use std::io::Read;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

/// A GET request (Python builds a `urllib.request.Request`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// For name lookup, connecting, sending the request and waiting for the response headers.
    pub timeout: Duration,
    /// For receiving the whole body (`None`: no limit).
    pub body_timeout: Option<Duration>,
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

impl HttpRequest {
    /// The first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// A 2xx response: its headers and the body, read as it arrives.
pub struct HttpResponse {
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    pub fn new(headers: Vec<(String, String)>, body: Box<dyn Read + Send>) -> Self {
        HttpResponse { headers, body }
    }

    /// The first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// Why there is no response.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The server answered with a status outside 200–299 (Python's `HTTPError`), including 304.
    #[error("HTTP {0}")]
    Status(u16),
    /// No answer: name lookup, connection, TLS or timeout (Python's `URLError` / `OSError`).
    #[error("{0}")]
    Connect(String),
}

pub trait HttpClient {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

impl<F> HttpClient for F
where
    F: Fn(&HttpRequest) -> Result<HttpResponse, HttpError>,
{
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self(request)
    }
}

/// The real network, through `ureq` with rustls and the operating system's trust store.
///
/// `timeout` covers name lookup, connecting, sending the request and waiting for the response
/// headers, `body_timeout` the whole body. (Python's socket timeout limits each read instead,
/// which ureq cannot express; a stalled transfer therefore ends at the body deadline.)
#[derive(Clone, Copy, Debug, Default)]
pub struct UreqClient;

impl UreqClient {
    fn agent(timeout: Duration, body_timeout: Option<Duration>) -> ureq::Agent {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_resolve(Some(timeout))
            .timeout_connect(Some(timeout))
            .timeout_send_request(Some(timeout))
            .timeout_recv_response(Some(timeout))
            .timeout_recv_body(body_timeout)
            .tls_config(tls)
            .build()
            .into()
    }
}

impl HttpClient for UreqClient {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut call = Self::agent(request.timeout, request.body_timeout).get(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name.as_str(), value.as_str());
        }
        let response = call
            .call()
            .map_err(|err| HttpError::Connect(err.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status(status));
        }
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                (name.as_str().to_string(), value)
            })
            .collect();
        let body = response.into_body().into_reader();
        Ok(HttpResponse::new(headers, Box::new(body)))
    }
}
