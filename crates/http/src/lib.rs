//! HTTP for a desktop client: the core's [`Transport`] (API calls, one GET each) and the engine's
//! [`ByteSource`] (audio, a ranged GET read as it comes) over one ureq agent with rustls, so API calls,
//! covers and audio share one connection pool and the radio wakes once for all of them.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use nori_engine::{Body, ByteSource};
use nori_core::transport::{FailureKind, Transport, TransportError, TransportResponse, USER_AGENT};
use ureq::Agent;

/// The largest API answer taken; a whole library page is far below it.
const MAX_ANSWER: u64 = 256 * 1024 * 1024;

pub struct Http {
    agent: Agent,
}

impl Http {
    pub fn new() -> Arc<Http> {
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build();
        Arc::new(Http { agent: config.into() })
    }
}

/// What went wrong, in the core's words for it.
fn failure(e: ureq::Error) -> TransportError {
    let kind = match &e {
        ureq::Error::HostNotFound => FailureKind::UnknownHost,
        ureq::Error::ConnectionFailed => FailureKind::Connect,
        ureq::Error::Timeout(_) => FailureKind::Timeout,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => FailureKind::Tls,
        ureq::Error::BadUri(_) | ureq::Error::Http(_) => FailureKind::Other,
        ureq::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => FailureKind::Connect,
            std::io::ErrorKind::TimedOut => FailureKind::Timeout,
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::UnexpectedEof => FailureKind::Interrupted,
            _ => FailureKind::Io,
        },
        _ => FailureKind::Io,
    };
    TransportError::Failed { kind, detail: Some(e.to_string()) }
}

/// Where a ranged answer starts, and the whole resource's length: `bytes 100-199/1000`.
fn content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let (range, total) = v.strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

#[async_trait::async_trait]
impl Transport for Http {
    /// Blocking inside: a desktop client drives the core's calls on threads of its own, and a future
    /// that completes when first polled needs no runtime.
    async fn get(&self, url: String, timeout_ms: u32) -> Result<TransportResponse, TransportError> {
        let mut req = self.agent.get(&url);
        if timeout_ms > 0 {
            req = req.config().timeout_global(Some(Duration::from_millis(timeout_ms as u64))).build();
        }
        let r = req.call().map_err(failure)?;
        let status = r.status().as_u16();
        let body = r.into_body().with_config().limit(MAX_ANSWER).read_to_vec().map_err(failure)?;
        Ok(TransportResponse { status, body })
    }

    fn address_changed(&self) {}
}

impl ByteSource for Http {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        let mut req = self.agent.get(url);
        if from > 0 {
            req = req.header("Range", format!("bytes={from}-"));
        }
        let r = req.call().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}"));
        }
        let header = |name: &str| r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        // A ranged answer says where it starts and how long the whole is; a plain one is the whole.
        let (start, len) = match header("content-range").as_deref().and_then(content_range) {
            Some(r) if status == 206 => r,
            _ => (0, header("content-length").and_then(|l| l.parse().ok())),
        };
        let reader: Box<dyn Read + Send> = Box::new(r.into_body().into_reader());
        Ok(Body { start, len, reader })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_range_says_where_the_bytes_start_and_how_many_there_are() {
        assert_eq!(content_range("bytes 100-199/1000"), Some((100, Some(1000))));
        assert_eq!(content_range("bytes 5-9/*"), Some((5, None)));
        assert_eq!(content_range("items 1-2/3"), None);
    }
}
