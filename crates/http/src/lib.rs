//! HTTP for a desktop client: the core's [`Transport`] (API calls, one GET each) and the engine's
//! [`ByteSource`] (audio, a ranged GET read as it comes) over one ureq agent with rustls, so API calls,
//! covers and audio share one connection pool and the radio wakes once for all of them.

use std::cell::RefCell;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nori_engine::{Body, ByteSource, Cancel, OpenError};
use nori_core::transport::{Exchange, FailureKind, Transport, TransportError, TransportResponse, USER_AGENT};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{Buffers, Connector, ConnectionDetails, DefaultConnector, NextTimeout};
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
        let agent = Agent::with_parts(config, DefaultConnector::new().chain(Callable), DefaultResolver::default());
        Arc::new(Http { agent })
    }

    /// `url`'s whole length, as a ranged answer for its first byte says it; None when the answer is not ranged.
    fn whole_length(&self, url: &str) -> Option<u64> {
        let r = self.agent.get(url).header("Range", "bytes=0-0").call().ok()?;
        if r.status().as_u16() != 206 {
            return None;
        }
        r.headers().get("content-range")?.to_str().ok().and_then(content_range)?.1
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

/// The whole length a range that could not be served names: `bytes */1000`.
fn unsatisfied_range(v: &str) -> Option<u64> {
    v.strip_prefix("bytes ")?.trim().strip_prefix("*/")?.trim().parse().ok()
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

    async fn send(&self, request: Exchange) -> Result<TransportResponse, TransportError> {
        let timeout = (request.timeout_ms > 0).then(|| Duration::from_millis(request.timeout_ms as u64));
        let r = match &request.json {
            Some(json) => {
                let mut req = self.agent.post(&request.url).header("Content-Type", "application/json");
                for (name, value) in &request.headers {
                    req = req.header(name, value);
                }
                if timeout.is_some() {
                    req = req.config().timeout_global(timeout).build();
                }
                req.send(json.as_bytes())
            }
            None => {
                let mut req = self.agent.get(&request.url);
                for (name, value) in &request.headers {
                    req = req.header(name, value);
                }
                if timeout.is_some() {
                    req = req.config().timeout_global(timeout).build();
                }
                req.call()
            }
        }
        .map_err(failure)?;
        let status = r.status().as_u16();
        let body = r.into_body().with_config().limit(MAX_ANSWER).read_to_vec().map_err(failure)?;
        Ok(TransportResponse { status, body })
    }

    fn address_changed(&self) {}
}

// ---- a song's request, called off ----

thread_local! {
    /// The request this thread waits on for the engine, while it does: its waits for bytes look at it.
    static CALLED: RefCell<Option<Cancel>> = const { RefCell::new(None) };
}

/// Runs `f` with `cancel` as this thread's request.
fn as_request<R>(cancel: &Cancel, f: impl FnOnce() -> R) -> R {
    let before = CALLED.with(|c| c.replace(Some(cancel.clone())));
    let r = f();
    CALLED.with(|c| *c.borrow_mut() = before);
    r
}

/// How often a wait for a song's bytes looks whether the engine has called its request off. ureq has no
/// way to cancel a call from another thread; its socket waits are cut into pieces this long instead, only
/// for the engine's requests and only while they wait.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// The last link of the connector chain: every connection's waits for input, on a thread that waits for
/// one of the engine's requests, end as soon as the engine calls that request off.
#[derive(Debug)]
struct Callable;

impl Connector<Box<dyn ureq::unversioned::transport::Transport>> for Callable {
    type Out = Called;

    fn connect(&self, _: &ConnectionDetails, chained: Option<Box<dyn ureq::unversioned::transport::Transport>>) -> Result<Option<Called>, ureq::Error> {
        Ok(chained.map(Called))
    }
}

#[derive(Debug)]
struct Called(Box<dyn ureq::unversioned::transport::Transport>);

impl ureq::unversioned::transport::Transport for Called {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let Some(cancel) = CALLED.with(|c| c.borrow().clone()) else { return self.0.await_input(timeout) };
        let until = (!timeout.after.is_not_happening()).then(|| Instant::now() + *timeout.after);
        loop {
            if cancel.cancelled() {
                return Err(ureq::Error::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "called off")));
            }
            let left = until.map(|u| u.saturating_duration_since(Instant::now()));
            let step = left.map_or(LOOK_EVERY, |l| l.min(LOOK_EVERY));
            let piece = NextTimeout { after: ureq::unversioned::transport::time::Duration::Exact(step), reason: timeout.reason };
            match self.0.await_input(piece) {
                Err(ureq::Error::Timeout(_)) if left.is_none_or(|l| l > step) => continue,
                other => return other,
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}

/// A song's body, read as the engine's request: a read waiting for bytes ends when it is called off.
struct Watched {
    inner: Box<dyn Read + Send>,
    cancel: Cancel,
}

impl Read for Watched {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (inner, cancel) = (&mut self.inner, &self.cancel);
        as_request(cancel, || inner.read(buf))
    }
}

impl ByteSource for Http {
    fn open(&self, url: &str, from: u64) -> Result<Body, OpenError> {
        self.open_cancellable(url, None, from, &Cancel::new())
    }

    fn open_cancellable(&self, url: &str, _key: Option<&str>, from: u64, cancel: &Cancel) -> Result<Body, OpenError> {
        let mut b = as_request(cancel, || self.open_now(url, from))?;
        b.reader = Box::new(Watched { inner: b.reader, cancel: cancel.clone() });
        Ok(b)
    }

    /// A station's stream, with its announcements asked for (`Icy-MetaData: 1`); the answer says how
    /// many bytes of music come between two (`icy-metaint`).
    fn open_live(&self, url: &str) -> Result<(Body, Option<usize>), String> {
        let r = self.agent.get(url).header("Icy-MetaData", "1").call().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}"));
        }
        let every = r.headers().get("icy-metaint").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
        let reader: Box<dyn Read + Send> = Box::new(r.into_body().into_reader());
        Ok((Body { start: 0, len: None, reader }, every))
    }
}

impl Http {
    /// `url` from byte `from` on, on this thread's request.
    fn open_now(&self, url: &str, from: u64) -> Result<Body, OpenError> {
        let mut req = self.agent.get(url);
        if from > 0 {
            req = req.header("Range", format!("bytes={from}-"));
        }
        let r = req.call().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        let header = |name: &str| r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        // A range from past the end (a length promised as an estimate): the answer says the real one.
        if status == 416 && from > 0 {
            // Without the length (a proxy drops Content-Range): the first byte alone, whose ranged answer says it.
            let len = header("content-range").as_deref().and_then(unsatisfied_range).or_else(|| self.whole_length(url)).filter(|&l| l <= from);
            return Err(OpenError::PastEnd { len });
        }
        if !(200..300).contains(&status) {
            return Err(OpenError::Status(status));
        }
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

    #[test]
    fn a_range_past_the_end_says_how_long_the_whole_is() {
        assert_eq!(unsatisfied_range("bytes */6406842"), Some(6_406_842));
        assert_eq!(unsatisfied_range("bytes */*"), None);
        assert_eq!(unsatisfied_range("bytes 0-9/10"), None);
    }
}
