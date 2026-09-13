//! HTTP for the desktop shell.
//!
//! The core is sans-IO, so fetching is this layer's job. Everything here is
//! blocking and runs on a worker thread; the interface thread never waits on
//! the network.

use std::fmt;
use std::io::Read;
use std::time::Duration;

use streamium_core::model::HttpHints;

/// User agent sent when the user has not chosen one. Some providers reject
/// clients they do not recognise, which is why [`crate::config::Config`]
/// lets this be overridden.
pub const DEFAULT_USER_AGENT: &str = concat!("Streamium/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub enum NetError {
    /// The server answered, but not with success.
    Status { code: u16, url: String },
    /// Nothing usable came back: DNS, TLS, connection, timeout.
    Transport(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::Status { code, url } => {
                let host = host_of(url);
                match code {
                    401 | 403 => write!(
                        f,
                        "{host} refused the request ({code}). Check the username and \
                         password, and whether the account allows another connection."
                    ),
                    404 => write!(f, "{host} has nothing at that address ({code})."),
                    429 => write!(f, "{host} is rate-limiting this account ({code})."),
                    500..=599 => write!(f, "{host} returned a server error ({code})."),
                    _ => write!(f, "{host} returned HTTP {code}."),
                }
            }
            NetError::Transport(e) => write!(f, "could not reach the server: {e}"),
        }
    }
}

impl std::error::Error for NetError {}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?']).next())
        .unwrap_or(url)
}

/// Build the shared agent. Redirects are followed because providers move
/// accounts between load balancers with 301s.
pub fn agent(user_agent: &str) -> ureq::Agent {
    let ua = if user_agent.trim().is_empty() {
        DEFAULT_USER_AGENT
    } else {
        user_agent.trim()
    };
    let config = ureq::Agent::config_builder()
        .user_agent(ua)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(180)))
        .max_redirects(5)
        .build();
    ureq::Agent::new_with_config(config)
}

/// An agent for pulling media, where the global timeout must not apply: a
/// live stream never ends on its own.
pub fn media_agent(user_agent: &str) -> ureq::Agent {
    let ua = if user_agent.trim().is_empty() {
        DEFAULT_USER_AGENT
    } else {
        user_agent.trim()
    };
    let config = ureq::Agent::config_builder()
        .user_agent(ua)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(None)
        .max_redirects(5)
        .build();
    ureq::Agent::new_with_config(config)
}

fn request(
    agent: &ureq::Agent,
    url: &str,
    hints: Option<&HttpHints>,
) -> Result<ureq::http::Response<ureq::Body>, NetError> {
    let mut req = agent.get(url);
    if let Some(h) = hints {
        if let Some(ua) = &h.user_agent {
            req = req.header("User-Agent", ua);
        }
        if let Some(r) = &h.referrer {
            req = req.header("Referer", r);
        }
        for (name, value) in &h.headers {
            req = req.header(name.as_str(), value.as_str());
        }
    }
    match req.call() {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::StatusCode(code)) => Err(NetError::Status {
            code,
            url: url.to_string(),
        }),
        Err(e) => Err(NetError::Transport(e.to_string())),
    }
}

/// Fetch a whole response as text, refusing anything larger than `limit`.
pub fn get_text(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String, NetError> {
    let mut resp = request(agent, url, None)?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| NetError::Transport(e.to_string()))
}

/// Open a response for streaming. Used for guides, which are far too large to
/// hold in memory twice.
pub fn get_reader(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
) -> Result<Box<dyn Read + Send>, NetError> {
    let resp = request(agent, url, None)?;
    Ok(Box::new(
        resp.into_body().into_with_config().limit(limit).reader(),
    ))
}

/// Open a media stream for reading, applying the per-channel HTTP hints that
/// playlists carry. Returns the reader and the `Content-Type`, which is how a
/// provider tells us it is handing back HLS rather than a transport stream.
pub fn open_stream(
    agent: &ureq::Agent,
    url: &str,
    hints: &HttpHints,
) -> Result<(Box<dyn Read + Send>, Option<String>), NetError> {
    let resp = request(agent, url, Some(hints))?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    Ok((Box::new(resp.into_body().into_reader()), content_type))
}
