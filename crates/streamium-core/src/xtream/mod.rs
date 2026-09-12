//! Xtream Codes API support.
//!
//! The "Xtream Codes" protocol is the de-facto standard used by IPTV
//! middleware (Xtream UI, XUI.one, StreamCreed, 1-Stream, ...). It consists of
//!
//! * `player_api.php` — a JSON API with an `action` query parameter,
//! * `xmltv.php`      — the XMLTV guide for the account,
//! * predictable stream URLs for live, VOD, series and time-shift.
//!
//! This module is sans-IO: [`Endpoints`] builds URLs, [`models`] decodes the
//! responses. The host performs the HTTP requests.

pub mod models;

use url::Url;

use crate::error::CoreError;

/// Container requested for live streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveContainer {
    /// Raw MPEG-TS over HTTP. Lowest latency; needs the transport-stream engine.
    Ts,
    /// HLS (`.m3u8`). Higher latency; plays in AVPlayer/ExoPlayer natively.
    Hls,
}

/// Builds every URL needed to talk to an Xtream Codes compatible server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    base: Url,
    username: String,
    password: String,
}

impl Endpoints {
    /// `base_url` may be given with or without a scheme, trailing slash, or a
    /// trailing `/player_api.php` / `/get.php` that users often paste in.
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self, CoreError> {
        let mut raw = base_url.trim().to_string();
        if raw.is_empty() {
            return Err(CoreError::InvalidUrl("empty server address".into()));
        }
        if !raw.contains("://") {
            raw = format!("http://{raw}");
        }
        let mut base = Url::parse(&raw)?;
        if base.host_str().is_none() || !(base.scheme() == "http" || base.scheme() == "https") {
            return Err(CoreError::InvalidUrl(format!(
                "unsupported server address: {base_url}"
            )));
        }
        // Normalise: drop query/fragment, drop a pasted script name, ensure trailing slash.
        base.set_query(None);
        base.set_fragment(None);
        let path = base.path().trim_end_matches('/').to_string();
        let path = [
            "/player_api.php",
            "/get.php",
            "/xmltv.php",
            "/panel_api.php",
        ]
        .iter()
        .fold(path, |p, script| {
            p.strip_suffix(script).map(str::to_string).unwrap_or(p)
        });
        base.set_path(&format!("{path}/"));
        if username.trim().is_empty() || password.is_empty() {
            return Err(CoreError::InvalidUrl(
                "username and password are required".into(),
            ));
        }
        Ok(Endpoints {
            base,
            username: username.trim().to_string(),
            password: password.to_string(),
        })
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str()
    }

    fn api(&self, action: Option<&str>, extra: &[(&str, &str)]) -> String {
        let mut u = self.base.join("player_api.php").expect("valid base");
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("username", &self.username)
                .append_pair("password", &self.password);
            if let Some(a) = action {
                q.append_pair("action", a);
            }
            for (k, v) in extra {
                q.append_pair(k, v);
            }
        }
        u.into()
    }

    /// Account + server info. Also the call used to validate credentials.
    pub fn account_info(&self) -> String {
        self.api(None, &[])
    }
    pub fn live_categories(&self) -> String {
        self.api(Some("get_live_categories"), &[])
    }
    pub fn live_streams(&self, category_id: Option<&str>) -> String {
        match category_id {
            Some(c) => self.api(Some("get_live_streams"), &[("category_id", c)]),
            None => self.api(Some("get_live_streams"), &[]),
        }
    }
    pub fn vod_categories(&self) -> String {
        self.api(Some("get_vod_categories"), &[])
    }
    pub fn vod_streams(&self, category_id: Option<&str>) -> String {
        match category_id {
            Some(c) => self.api(Some("get_vod_streams"), &[("category_id", c)]),
            None => self.api(Some("get_vod_streams"), &[]),
        }
    }
    pub fn vod_info(&self, vod_id: &str) -> String {
        self.api(Some("get_vod_info"), &[("vod_id", vod_id)])
    }
    pub fn series_categories(&self) -> String {
        self.api(Some("get_series_categories"), &[])
    }
    pub fn series(&self, category_id: Option<&str>) -> String {
        match category_id {
            Some(c) => self.api(Some("get_series"), &[("category_id", c)]),
            None => self.api(Some("get_series"), &[]),
        }
    }
    pub fn series_info(&self, series_id: &str) -> String {
        self.api(Some("get_series_info"), &[("series_id", series_id)])
    }
    /// Short EPG (next few programmes) for one channel, base64-encoded titles.
    pub fn short_epg(&self, stream_id: &str, limit: Option<u32>) -> String {
        match limit {
            Some(l) => {
                let l = l.to_string();
                self.api(
                    Some("get_short_epg"),
                    &[("stream_id", stream_id), ("limit", &l)],
                )
            }
            None => self.api(Some("get_short_epg"), &[("stream_id", stream_id)]),
        }
    }
    /// Full EPG table for one channel.
    pub fn full_epg(&self, stream_id: &str) -> String {
        self.api(Some("get_simple_data_table"), &[("stream_id", stream_id)])
    }
    /// XMLTV document for the whole account.
    pub fn xmltv(&self) -> String {
        let mut u = self.base.join("xmltv.php").expect("valid base");
        u.query_pairs_mut()
            .append_pair("username", &self.username)
            .append_pair("password", &self.password);
        u.into()
    }

    pub fn live_stream_url(&self, stream_id: &str, container: LiveContainer) -> String {
        let ext = match container {
            LiveContainer::Ts => "ts",
            LiveContainer::Hls => "m3u8",
        };
        self.media_url("live", stream_id, ext)
    }
    pub fn movie_url(&self, stream_id: &str, container_extension: &str) -> String {
        self.media_url("movie", stream_id, container_extension)
    }
    pub fn episode_url(&self, episode_id: &str, container_extension: &str) -> String {
        self.media_url("series", episode_id, container_extension)
    }

    fn media_url(&self, kind: &str, id: &str, ext: &str) -> String {
        let ext = ext.trim().trim_start_matches('.');
        let ext = if ext.is_empty() { "ts" } else { ext };
        let mut u = self.base.clone();
        {
            let mut segs = u.path_segments_mut().expect("base is not cannot-be-a-base");
            segs.pop_if_empty();
            segs.push(kind)
                .push(&self.username)
                .push(&self.password)
                .push(&format!("{id}.{ext}"));
        }
        u.into()
    }

    /// Time-shift (catch-up) URL. `start_unix` is the programme start in UTC
    /// seconds; `duration_minutes` how much to play from there.
    pub fn timeshift_url(&self, stream_id: &str, start_unix: i64, duration_minutes: u32) -> String {
        let start = format_timeshift_start(start_unix);
        let mut u = self
            .base
            .join("streaming/timeshift.php")
            .expect("valid base");
        u.query_pairs_mut()
            .append_pair("username", &self.username)
            .append_pair("password", &self.password)
            .append_pair("stream", stream_id)
            .append_pair("start", &start)
            .append_pair("duration", &duration_minutes.to_string());
        u.into()
    }
}

/// Xtream expects `YYYY-MM-DD:HH-MM` in the server's local time. Servers are
/// overwhelmingly configured in UTC; a per-source offset can be added later.
fn format_timeshift_start(unix: i64) -> String {
    let (y, m, d, hh, mm, _ss) = crate::xmltv::civil_from_unix(unix);
    format!("{y:04}-{m:02}-{d:02}:{hh:02}-{mm:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_pasted_urls() {
        let e = Endpoints::new("example.com:8080/player_api.php?username=a", "u", "p").unwrap();
        assert_eq!(e.base_url(), "http://example.com:8080/");
        let e = Endpoints::new("https://tv.example/sub/get.php", "u", "p").unwrap();
        assert_eq!(e.base_url(), "https://tv.example/sub/");
        let e = Endpoints::new("https://tv.example/", " u ", "p").unwrap();
        assert_eq!(
            e.account_info(),
            "https://tv.example/player_api.php?username=u&password=p"
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Endpoints::new("", "u", "p").is_err());
        assert!(Endpoints::new("ftp://x", "u", "p").is_err());
        assert!(Endpoints::new("http://x", "", "p").is_err());
    }

    #[test]
    fn builds_api_and_media_urls() {
        let e = Endpoints::new("http://h:80", "us er", "p&w").unwrap();
        assert_eq!(
            e.live_streams(Some("7")),
            "http://h/player_api.php?username=us+er&password=p%26w&action=get_live_streams&category_id=7"
        );
        assert_eq!(
            e.live_stream_url("12", LiveContainer::Ts),
            "http://h/live/us%20er/p&w/12.ts"
        );
        assert_eq!(
            e.live_stream_url("12", LiveContainer::Hls),
            "http://h/live/us%20er/p&w/12.m3u8"
        );
        assert_eq!(e.movie_url("5", "mkv"), "http://h/movie/us%20er/p&w/5.mkv");
        assert_eq!(
            e.episode_url("9", ".mp4"),
            "http://h/series/us%20er/p&w/9.mp4"
        );
        assert_eq!(
            e.xmltv(),
            "http://h/xmltv.php?username=us+er&password=p%26w"
        );
        assert_eq!(
            e.short_epg("3", Some(4)),
            "http://h/player_api.php?username=us+er&password=p%26w&action=get_short_epg&stream_id=3&limit=4"
        );
    }

    #[test]
    fn timeshift_url_formats_start() {
        let e = Endpoints::new("http://h", "u", "p").unwrap();
        // 2024-03-05 18:30:00 UTC
        let url = e.timeshift_url("77", 1_709_663_400, 90);
        assert_eq!(
            url,
            "http://h/streaming/timeshift.php?username=u&password=p&stream=77&start=2024-03-05%3A18-30&duration=90"
        );
    }
}
