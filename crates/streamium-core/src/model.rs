//! Domain model shared by every feature of the player.

use serde::{Deserialize, Serialize};

/// What kind of media a stream carries. Drives which playback engine is used
/// and how the UI presents the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    /// Linear television.
    Live,
    /// Video on demand (a movie or one-off item).
    Movie,
    /// An episode belonging to a series.
    Episode,
    /// Audio-only stream (internet radio, podcast).
    Radio,
    /// Media the user owns: local file, network share, personal media server.
    Personal,
}

/// A hint about the container/protocol of a stream URL, derived purely from
/// the URL. The host uses this to pick a playback engine *before* opening the
/// connection. Sniffing the first bytes may still override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamFormat {
    /// HTTP Live Streaming (`.m3u8`). Natively supported by AVPlayer/ExoPlayer.
    Hls,
    /// MPEG-DASH manifest (`.mpd`).
    Dash,
    /// Raw MPEG transport stream over HTTP (`.ts`, or extension-less Xtream live URLs).
    MpegTs,
    /// Progressive file (`.mp4`, `.mkv`, `.avi`, `.mp3`, ...).
    Progressive,
    /// RTSP/RTP/UDP multicast — requires the fallback engine.
    Rtsp,
    Udp,
    /// Could not tell from the URL alone; sniff the payload.
    Unknown,
}

impl StreamFormat {
    /// Infer the format from a URL. This is intentionally conservative: when in
    /// doubt it returns [`StreamFormat::Unknown`] so the host sniffs the payload.
    pub fn infer(url: &str) -> StreamFormat {
        let lower = url.trim().to_ascii_lowercase();
        if lower.starts_with("rtsp://") || lower.starts_with("rtsps://") {
            return StreamFormat::Rtsp;
        }
        if lower.starts_with("udp://") || lower.starts_with("rtp://") {
            return StreamFormat::Udp;
        }
        // Strip query string and fragment before looking at the extension.
        let path = lower.split(['?', '#']).next().unwrap_or("");
        let last_segment = path.rsplit('/').next().unwrap_or("");
        let ext = last_segment.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "m3u8" | "m3u" => StreamFormat::Hls,
            "mpd" => StreamFormat::Dash,
            "ts" | "m2ts" | "mts" => StreamFormat::MpegTs,
            "mp4" | "m4v" | "mov" | "mkv" | "avi" | "webm" | "flv" | "wmv" | "mp3" | "aac"
            | "flac" | "ogg" | "m4a" | "wav" | "opus" => StreamFormat::Progressive,
            _ => {
                // Xtream live URLs often have no extension: /live/user/pass/123
                if path.contains("/live/") && ext.is_empty() {
                    StreamFormat::MpegTs
                } else {
                    StreamFormat::Unknown
                }
            }
        }
    }
}

/// Catch-up (time-shift) capability advertised by a playlist entry or provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Catchup {
    /// Scheme name as advertised (`default`, `append`, `shift`, `flussonic`, `xc`, ...).
    pub kind: String,
    /// Template used to construct a time-shifted URL, if given.
    pub source: Option<String>,
    /// Number of days of archive available.
    pub days: Option<u32>,
}

/// Per-stream HTTP hints. IPTV playlists commonly carry these as
/// `#EXTVLCOPT` or `#KODIPROP` lines; some providers refuse connections
/// without the exact User-Agent they were provisioned for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HttpHints {
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    /// Additional headers, `name: value` pairs.
    pub headers: Vec<(String, String)>,
}

/// One playable item as it appears in a playlist or provider listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Stable identifier inside its source (playlist line index or Xtream stream id).
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: MediaKind,
    pub format: StreamFormat,
    pub group: Option<String>,
    pub logo: Option<String>,
    /// XMLTV channel id (`tvg-id`) used to join with the EPG.
    pub epg_id: Option<String>,
    /// Channel number for the zap bar and remote control entry.
    pub number: Option<u32>,
    pub catchup: Option<Catchup>,
    pub http: HttpHints,
}

impl Channel {
    pub fn new(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        let url = url.into();
        let format = StreamFormat::infer(&url);
        Channel {
            id: id.into(),
            name: name.into(),
            url,
            kind: MediaKind::Live,
            format,
            group: None,
            logo: None,
            epg_id: None,
            number: None,
            catchup: None,
            http: HttpHints::default(),
        }
    }
}

/// Where a catalog comes from. Streamium never ships content: every source is
/// entered by the user, who is responsible for holding the rights to use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// A remote or local M3U/M3U8 playlist.
    Playlist {
        name: String,
        url: String,
        /// Optional XMLTV URL for the guide. If absent, `url-tvg` from the playlist header is used.
        epg_url: Option<String>,
    },
    /// An Xtream Codes compatible server.
    Xtream {
        name: String,
        base_url: String,
        username: String,
        password: String,
    },
    /// A local folder or platform media library (Files, Photos, external drive).
    LocalLibrary { name: String, path: String },
    /// A personal media server the user runs (Jellyfin, Plex, Emby, Tvheadend, HDHomeRun).
    MediaServer {
        name: String,
        server_kind: String,
        base_url: String,
    },
}

impl Source {
    pub fn name(&self) -> &str {
        match self {
            Source::Playlist { name, .. }
            | Source::Xtream { name, .. }
            | Source::LocalLibrary { name, .. }
            | Source::MediaServer { name, .. } => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_common_formats() {
        assert_eq!(
            StreamFormat::infer("http://h/a/b.m3u8?token=1"),
            StreamFormat::Hls
        );
        assert_eq!(StreamFormat::infer("HTTPS://H/a.M3U8"), StreamFormat::Hls);
        assert_eq!(
            StreamFormat::infer("http://h/live/u/p/12.ts"),
            StreamFormat::MpegTs
        );
        assert_eq!(
            StreamFormat::infer("http://h/live/u/p/12"),
            StreamFormat::MpegTs
        );
        assert_eq!(
            StreamFormat::infer("http://h/movie/u/p/12.mkv"),
            StreamFormat::Progressive
        );
        assert_eq!(
            StreamFormat::infer("rtsp://cam/stream1"),
            StreamFormat::Rtsp
        );
        assert_eq!(
            StreamFormat::infer("udp://@239.0.0.1:1234"),
            StreamFormat::Udp
        );
        assert_eq!(
            StreamFormat::infer("http://h/manifest.mpd"),
            StreamFormat::Dash
        );
        assert_eq!(
            StreamFormat::infer("http://h/whatever"),
            StreamFormat::Unknown
        );
    }
}
