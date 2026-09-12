//! # streamium-ffi
//!
//! The single crate that crosses the language boundary. It re-exposes the
//! sans-IO core and the transport-stream demuxer through UniFFI so that the
//! Swift (iOS, iPadOS, macOS, tvOS) and Kotlin (Android) shells call one
//! identical implementation. Desktop shells written in Rust link the core
//! crates directly and skip this layer.
//!
//! Conventions:
//! * Records are plain value types (Swift structs / Kotlin data classes).
//! * Objects (`Catalog`, `EpgGuide`, `TransportStreamDemuxer`, `XtreamEndpoints`)
//!   are reference types that own state; they are `Send + Sync` and internally
//!   synchronised, so the host can call them from any queue.
//! * Every fallible call returns [`FfiError`]. The core never panics across
//!   the boundary (`panic = "abort"` in release makes that a hard guarantee).

use std::sync::{Arc, Mutex, RwLock};

use streamium_core::catalog::Catalog;
use streamium_core::epg::{EpgIndex, Programme};
use streamium_core::m3u;
use streamium_core::model::{Catchup, Channel, HttpHints, MediaKind, StreamFormat};
use streamium_core::xmltv;
use streamium_core::xtream::models as xm;
use streamium_core::xtream::{Endpoints, LiveContainer};
use streamium_core::CoreError;
use streamium_mpegts as ts;

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum FfiError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("not a playlist")]
    NotAPlaylist,
    #[error("parse error: {0}")]
    Parse(String),
    #[error("{0}")]
    Other(String),
}

impl From<CoreError> for FfiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::InvalidUrl(s) => FfiError::InvalidUrl(s),
            CoreError::NotAPlaylist => FfiError::NotAPlaylist,
            CoreError::Xmltv(s) | CoreError::Json(s) | CoreError::Decompress(s) => {
                FfiError::Parse(s)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Records and enums mirrored from streamium-core
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiMediaKind {
    Live,
    Movie,
    Episode,
    Radio,
    Personal,
}

impl From<MediaKind> for FfiMediaKind {
    fn from(k: MediaKind) -> Self {
        match k {
            MediaKind::Live => FfiMediaKind::Live,
            MediaKind::Movie => FfiMediaKind::Movie,
            MediaKind::Episode => FfiMediaKind::Episode,
            MediaKind::Radio => FfiMediaKind::Radio,
            MediaKind::Personal => FfiMediaKind::Personal,
        }
    }
}
impl From<FfiMediaKind> for MediaKind {
    fn from(k: FfiMediaKind) -> Self {
        match k {
            FfiMediaKind::Live => MediaKind::Live,
            FfiMediaKind::Movie => MediaKind::Movie,
            FfiMediaKind::Episode => MediaKind::Episode,
            FfiMediaKind::Radio => MediaKind::Radio,
            FfiMediaKind::Personal => MediaKind::Personal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiStreamFormat {
    Hls,
    Dash,
    MpegTs,
    Progressive,
    Rtsp,
    Udp,
    Unknown,
}

impl From<StreamFormat> for FfiStreamFormat {
    fn from(f: StreamFormat) -> Self {
        match f {
            StreamFormat::Hls => FfiStreamFormat::Hls,
            StreamFormat::Dash => FfiStreamFormat::Dash,
            StreamFormat::MpegTs => FfiStreamFormat::MpegTs,
            StreamFormat::Progressive => FfiStreamFormat::Progressive,
            StreamFormat::Rtsp => FfiStreamFormat::Rtsp,
            StreamFormat::Udp => FfiStreamFormat::Udp,
            StreamFormat::Unknown => FfiStreamFormat::Unknown,
        }
    }
}
impl From<FfiStreamFormat> for StreamFormat {
    fn from(f: FfiStreamFormat) -> Self {
        match f {
            FfiStreamFormat::Hls => StreamFormat::Hls,
            FfiStreamFormat::Dash => StreamFormat::Dash,
            FfiStreamFormat::MpegTs => StreamFormat::MpegTs,
            FfiStreamFormat::Progressive => StreamFormat::Progressive,
            FfiStreamFormat::Rtsp => StreamFormat::Rtsp,
            FfiStreamFormat::Udp => StreamFormat::Udp,
            FfiStreamFormat::Unknown => StreamFormat::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiChannel {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: FfiMediaKind,
    pub format: FfiStreamFormat,
    pub group: Option<String>,
    pub logo: Option<String>,
    pub epg_id: Option<String>,
    pub number: Option<u32>,
    pub catchup_kind: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_days: Option<u32>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    pub headers: Vec<FfiHeader>,
}

impl From<&Channel> for FfiChannel {
    fn from(c: &Channel) -> Self {
        FfiChannel {
            id: c.id.clone(),
            name: c.name.clone(),
            url: c.url.clone(),
            kind: c.kind.into(),
            format: c.format.into(),
            group: c.group.clone(),
            logo: c.logo.clone(),
            epg_id: c.epg_id.clone(),
            number: c.number,
            catchup_kind: c.catchup.as_ref().map(|x| x.kind.clone()),
            catchup_source: c.catchup.as_ref().and_then(|x| x.source.clone()),
            catchup_days: c.catchup.as_ref().and_then(|x| x.days),
            user_agent: c.http.user_agent.clone(),
            referrer: c.http.referrer.clone(),
            headers: c
                .http
                .headers
                .iter()
                .map(|(n, v)| FfiHeader {
                    name: n.clone(),
                    value: v.clone(),
                })
                .collect(),
        }
    }
}

impl From<FfiChannel> for Channel {
    fn from(c: FfiChannel) -> Self {
        Channel {
            id: c.id,
            name: c.name,
            url: c.url,
            kind: c.kind.into(),
            format: c.format.into(),
            group: c.group,
            logo: c.logo,
            epg_id: c.epg_id,
            number: c.number,
            catchup: c.catchup_kind.map(|kind| Catchup {
                kind,
                source: c.catchup_source,
                days: c.catchup_days,
            }),
            http: HttpHints {
                user_agent: c.user_agent,
                referrer: c.referrer,
                headers: c.headers.into_iter().map(|h| (h.name, h.value)).collect(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiPlaylist {
    /// True when the document was an HLS media/master playlist for a single stream.
    pub is_hls_media: bool,
    pub channels: Vec<FfiChannel>,
    pub epg_urls: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiProgramme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

impl From<&Programme> for FfiProgramme {
    fn from(p: &Programme) -> Self {
        FfiProgramme {
            start: p.start,
            stop: p.stop,
            title: p.title.clone(),
            subtitle: p.subtitle.clone(),
            description: p.description.clone(),
            category: p.category.clone(),
            icon: p.icon.clone(),
            season: p.season,
            episode: p.episode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiNowNext {
    pub now: Option<FfiProgramme>,
    pub next: Option<FfiProgramme>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiProgrammeHit {
    pub channel_id: String,
    pub programme: FfiProgramme,
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

/// Parse an M3U/M3U8 document into channels.
#[uniffi::export]
pub fn parse_playlist(text: String) -> Result<FfiPlaylist, FfiError> {
    let p = m3u::parse(&text)?;
    Ok(FfiPlaylist {
        is_hls_media: p.kind == m3u::PlaylistKind::HlsMedia,
        channels: p.channels.iter().map(FfiChannel::from).collect(),
        epg_urls: p.epg_urls,
        warnings: p
            .warnings
            .into_iter()
            .map(|(line, msg)| format!("line {line}: {msg}"))
            .collect(),
    })
}

/// Guess the container/protocol of a stream URL so the host can pick an engine.
#[uniffi::export]
pub fn infer_stream_format(url: String) -> FfiStreamFormat {
    StreamFormat::infer(&url).into()
}

// ---------------------------------------------------------------------------
// Xtream Codes
// ---------------------------------------------------------------------------

#[derive(Debug, uniffi::Object)]
pub struct XtreamEndpoints {
    inner: Endpoints,
}

#[uniffi::export]
impl XtreamEndpoints {
    #[uniffi::constructor]
    pub fn new(
        base_url: String,
        username: String,
        password: String,
    ) -> Result<Arc<Self>, FfiError> {
        Ok(Arc::new(XtreamEndpoints {
            inner: Endpoints::new(&base_url, &username, &password)?,
        }))
    }
    pub fn base_url(&self) -> String {
        self.inner.base_url().to_string()
    }
    pub fn account_info(&self) -> String {
        self.inner.account_info()
    }
    pub fn live_categories(&self) -> String {
        self.inner.live_categories()
    }
    pub fn live_streams(&self, category_id: Option<String>) -> String {
        self.inner.live_streams(category_id.as_deref())
    }
    pub fn vod_categories(&self) -> String {
        self.inner.vod_categories()
    }
    pub fn vod_streams(&self, category_id: Option<String>) -> String {
        self.inner.vod_streams(category_id.as_deref())
    }
    pub fn vod_info(&self, vod_id: String) -> String {
        self.inner.vod_info(&vod_id)
    }
    pub fn series_categories(&self) -> String {
        self.inner.series_categories()
    }
    pub fn series(&self, category_id: Option<String>) -> String {
        self.inner.series(category_id.as_deref())
    }
    pub fn series_info(&self, series_id: String) -> String {
        self.inner.series_info(&series_id)
    }
    pub fn short_epg(&self, stream_id: String, limit: Option<u32>) -> String {
        self.inner.short_epg(&stream_id, limit)
    }
    pub fn full_epg(&self, stream_id: String) -> String {
        self.inner.full_epg(&stream_id)
    }
    pub fn xmltv(&self) -> String {
        self.inner.xmltv()
    }
    pub fn live_stream_url(&self, stream_id: String, hls: bool) -> String {
        let c = if hls {
            LiveContainer::Hls
        } else {
            LiveContainer::Ts
        };
        self.inner.live_stream_url(&stream_id, c)
    }
    pub fn movie_url(&self, stream_id: String, container_extension: String) -> String {
        self.inner.movie_url(&stream_id, &container_extension)
    }
    pub fn episode_url(&self, episode_id: String, container_extension: String) -> String {
        self.inner.episode_url(&episode_id, &container_extension)
    }
    pub fn timeshift_url(
        &self,
        stream_id: String,
        start_unix: i64,
        duration_minutes: u32,
    ) -> String {
        self.inner
            .timeshift_url(&stream_id, start_unix, duration_minutes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiAccount {
    pub username: Option<String>,
    pub status: Option<String>,
    pub is_active: bool,
    pub expires_unix: Option<i64>,
    pub is_trial: bool,
    pub active_connections: Option<i64>,
    pub max_connections: Option<i64>,
    pub server_timezone: Option<String>,
    pub server_time_unix: Option<i64>,
    pub allowed_output_formats: Vec<String>,
}

#[uniffi::export]
pub fn xtream_decode_account(json: String) -> Result<FfiAccount, FfiError> {
    let a: xm::AccountInfo = xm::decode_optional(&json)?
        .ok_or_else(|| FfiError::Parse("empty account response".into()))?;
    Ok(FfiAccount {
        username: a.user_info.username.clone(),
        status: a.user_info.status.clone(),
        is_active: a.user_info.is_active(),
        expires_unix: a.user_info.exp_date,
        is_trial: a.user_info.is_trial.unwrap_or(false),
        active_connections: a.user_info.active_cons,
        max_connections: a.user_info.max_connections,
        server_timezone: a.server_info.timezone,
        server_time_unix: a.server_info.timestamp_now,
        allowed_output_formats: a.user_info.allowed_output_formats,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiCategory {
    pub id: String,
    pub name: String,
}

#[uniffi::export]
pub fn xtream_decode_categories(json: String) -> Result<Vec<FfiCategory>, FfiError> {
    let cats: Vec<xm::Category> = xm::decode_optional(&json)?.unwrap_or_default();
    Ok(cats
        .into_iter()
        .filter_map(|c| {
            Some(FfiCategory {
                id: c.category_id?,
                name: c.category_name.unwrap_or_default(),
            })
        })
        .collect())
}

/// Turn `get_live_categories` + `get_live_streams` responses into channels.
#[uniffi::export]
pub fn xtream_live_channels(
    endpoints: &XtreamEndpoints,
    categories_json: String,
    streams_json: String,
    hls: bool,
) -> Result<Vec<FfiChannel>, FfiError> {
    let cats: Vec<xm::Category> = xm::decode_optional(&categories_json)?.unwrap_or_default();
    let streams: Vec<xm::LiveStream> = xm::decode_optional(&streams_json)?.unwrap_or_default();
    let container = if hls {
        LiveContainer::Hls
    } else {
        LiveContainer::Ts
    };
    let cat = Catalog::from_xtream_live(&endpoints.inner, &cats, &streams, container);
    Ok(cat.channels().iter().map(FfiChannel::from).collect())
}

/// Turn `get_vod_categories` + `get_vod_streams` responses into movie items.
#[uniffi::export]
pub fn xtream_vod_channels(
    endpoints: &XtreamEndpoints,
    categories_json: String,
    streams_json: String,
) -> Result<Vec<FfiChannel>, FfiError> {
    let cats: Vec<xm::Category> = xm::decode_optional(&categories_json)?.unwrap_or_default();
    let streams: Vec<xm::VodStream> = xm::decode_optional(&streams_json)?.unwrap_or_default();
    let cat = Catalog::from_xtream_vod(&endpoints.inner, &cats, &streams);
    Ok(cat.channels().iter().map(FfiChannel::from).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiSeries {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
    pub plot: Option<String>,
    pub genre: Option<String>,
    pub release_date: Option<String>,
    pub category_id: Option<String>,
}

#[uniffi::export]
pub fn xtream_decode_series_list(json: String) -> Result<Vec<FfiSeries>, FfiError> {
    let list: Vec<xm::SeriesListing> = xm::decode_optional(&json)?.unwrap_or_default();
    Ok(list.into_iter().filter_map(series_from).collect())
}

fn series_from(s: xm::SeriesListing) -> Option<FfiSeries> {
    Some(FfiSeries {
        id: s.series_id?,
        name: s.name.unwrap_or_default(),
        cover: s.cover,
        plot: s.plot,
        genre: s.genre,
        release_date: s.release_date,
        category_id: s.category_id,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiEpisode {
    pub id: String,
    pub season: u32,
    pub number: u32,
    pub title: String,
    pub url: String,
    pub plot: Option<String>,
    pub image: Option<String>,
    pub duration_seconds: Option<u32>,
}

/// Decode `get_series_info` into playable episodes with URLs filled in.
#[uniffi::export]
pub fn xtream_decode_series_info(
    endpoints: &XtreamEndpoints,
    json: String,
) -> Result<Vec<FfiEpisode>, FfiError> {
    let info: xm::SeriesInfo = xm::decode_optional(&json)?.unwrap_or_default();
    Ok(info
        .episodes
        .into_iter()
        .filter_map(|e| {
            let id = e.id?;
            let ext = e.container_extension.as_deref().unwrap_or("mp4");
            Some(FfiEpisode {
                url: endpoints.inner.episode_url(&id, ext),
                id,
                season: e.season.and_then(|s| u32::try_from(s).ok()).unwrap_or(0),
                number: e
                    .episode_num
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                title: e.title.unwrap_or_default(),
                plot: e.info.as_ref().and_then(|i| i.plot.clone()),
                image: e.info.as_ref().and_then(|i| i.movie_image.clone()),
                duration_seconds: e
                    .info
                    .as_ref()
                    .and_then(|i| i.duration_secs)
                    .and_then(|d| u32::try_from(d).ok()),
            })
        })
        .collect())
}

/// Decode `get_short_epg` / `get_simple_data_table` into programmes (titles decoded from base64).
#[uniffi::export]
pub fn xtream_decode_epg(json: String) -> Result<Vec<FfiProgramme>, FfiError> {
    let r: xm::EpgResponse = xm::decode_optional(&json)?.unwrap_or_default();
    Ok(r.epg_listings
        .iter()
        .filter_map(|l| {
            Some(FfiProgramme {
                start: l.start_timestamp?,
                stop: l.stop_timestamp?,
                title: l.title_text(),
                subtitle: None,
                description: Some(l.description_text()).filter(|d| !d.is_empty()),
                category: None,
                icon: None,
                season: None,
                episode: None,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// EPG
// ---------------------------------------------------------------------------

/// A thread-safe programme guide. Load one or more XMLTV documents into it,
/// then query now/next and time ranges from any thread.
#[derive(Debug, Default, uniffi::Object)]
pub struct EpgGuide {
    index: RwLock<EpgIndex>,
}

#[uniffi::export]
impl EpgGuide {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Parse an XMLTV document (plain or gzip) and merge it in. Returns the
    /// number of programmes now held.
    pub fn load_xmltv(&self, bytes: Vec<u8>) -> Result<u64, FfiError> {
        let parsed = xmltv::parse(&bytes)?;
        let mut idx = self.index.write().unwrap_or_else(|p| p.into_inner());
        if idx.programme_count() == 0 {
            *idx = parsed;
        } else {
            idx.merge(parsed);
        }
        Ok(idx.programme_count() as u64)
    }

    pub fn clear(&self) {
        *self.index.write().unwrap_or_else(|p| p.into_inner()) = EpgIndex::new();
    }
    pub fn channel_count(&self) -> u64 {
        self.index
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .channel_count() as u64
    }
    pub fn programme_count(&self) -> u64 {
        self.index
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .programme_count() as u64
    }
    pub fn has_programmes(&self, channel_id: String) -> bool {
        self.index
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .has_programmes(&channel_id)
    }
    /// Map a playlist entry to an EPG channel id (by `tvg-id`, then by name).
    pub fn resolve(&self, tvg_id: Option<String>, name: String) -> Option<String> {
        self.index
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .resolve(tvg_id.as_deref(), &name)
    }
    pub fn now_next(&self, channel_id: String, at_unix: i64) -> FfiNowNext {
        let idx = self.index.read().unwrap_or_else(|p| p.into_inner());
        let (now, next) = idx.now_next(&channel_id, at_unix);
        FfiNowNext {
            now: now.map(FfiProgramme::from),
            next: next.map(FfiProgramme::from),
        }
    }
    pub fn between(&self, channel_id: String, from_unix: i64, to_unix: i64) -> Vec<FfiProgramme> {
        let idx = self.index.read().unwrap_or_else(|p| p.into_inner());
        idx.between(&channel_id, from_unix, to_unix)
            .iter()
            .map(FfiProgramme::from)
            .collect()
    }
    pub fn search(&self, query: String, after_unix: i64, limit: u32) -> Vec<FfiProgrammeHit> {
        let idx = self.index.read().unwrap_or_else(|p| p.into_inner());
        idx.search(&query, after_unix, limit as usize)
            .into_iter()
            .map(|(id, p)| FfiProgrammeHit {
                channel_id: id.to_string(),
                programme: p.into(),
            })
            .collect()
    }
    pub fn prune_before(&self, before_unix: i64) {
        self.index
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .prune_before(before_unix);
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// The user's merged channel list with groups, search and favourites.
#[derive(Debug, Default, uniffi::Object)]
pub struct ChannelCatalog {
    inner: RwLock<Catalog>,
}

#[uniffi::export]
impl ChannelCatalog {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Restore a catalog previously saved with [`ChannelCatalog::to_json`].
    #[uniffi::constructor]
    pub fn from_json(json: String) -> Result<Arc<Self>, FfiError> {
        let mut cat: Catalog =
            serde_json::from_str(&json).map_err(|e| FfiError::Parse(e.to_string()))?;
        cat.reindex();
        Ok(Arc::new(ChannelCatalog {
            inner: RwLock::new(cat),
        }))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&*self.inner.read().unwrap_or_else(|p| p.into_inner()))
            .unwrap_or_default()
    }

    pub fn add_channels(&self, channels: Vec<FfiChannel>) {
        let mut cat = self.inner.write().unwrap_or_else(|p| p.into_inner());
        for c in channels {
            cat.push(c.into());
        }
    }

    pub fn clear(&self) {
        *self.inner.write().unwrap_or_else(|p| p.into_inner()) = Catalog::new();
    }

    pub fn count(&self) -> u64 {
        self.inner.read().unwrap_or_else(|p| p.into_inner()).len() as u64
    }
    pub fn channels(&self) -> Vec<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .channels()
            .iter()
            .map(FfiChannel::from)
            .collect()
    }
    pub fn groups(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .groups()
            .to_vec()
    }
    pub fn in_group(&self, group: String) -> Vec<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .in_group(&group)
            .map(FfiChannel::from)
            .collect()
    }
    pub fn of_kind(&self, kind: FfiMediaKind) -> Vec<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .of_kind(kind.into())
            .map(FfiChannel::from)
            .collect()
    }
    pub fn get(&self, id: String) -> Option<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .map(FfiChannel::from)
    }
    pub fn by_number(&self, number: u32) -> Option<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .by_number(number)
            .map(FfiChannel::from)
    }
    pub fn search(&self, query: String, limit: u32) -> Vec<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .search(&query, limit as usize)
            .into_iter()
            .map(|h| FfiChannel::from(h.channel))
            .collect()
    }
    pub fn is_favourite(&self, id: String) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_favourite(&id)
    }
    pub fn set_favourite(&self, id: String, on: bool) {
        self.inner
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .set_favourite(&id, on);
    }
    pub fn favourites(&self) -> Vec<FfiChannel> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .favourites()
            .map(FfiChannel::from)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Transport stream demuxer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiCodec {
    H264,
    H265,
    Mpeg2Video,
    AacAdts,
    AacLatm,
    MpegAudio,
    Ac3,
    Eac3,
    Dts,
    DvbSubtitle,
    Teletext,
    Other,
}

impl From<ts::Codec> for FfiCodec {
    fn from(c: ts::Codec) -> Self {
        match c {
            ts::Codec::H264 => FfiCodec::H264,
            ts::Codec::H265 => FfiCodec::H265,
            ts::Codec::Mpeg2Video => FfiCodec::Mpeg2Video,
            ts::Codec::AacAdts => FfiCodec::AacAdts,
            ts::Codec::AacLatm => FfiCodec::AacLatm,
            ts::Codec::MpegAudio => FfiCodec::MpegAudio,
            ts::Codec::Ac3 => FfiCodec::Ac3,
            ts::Codec::Eac3 => FfiCodec::Eac3,
            ts::Codec::Dts => FfiCodec::Dts,
            ts::Codec::DvbSubtitle => FfiCodec::DvbSubtitle,
            ts::Codec::Teletext => FfiCodec::Teletext,
            ts::Codec::Other(_) => FfiCodec::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiStream {
    pub pid: u16,
    pub codec: FfiCodec,
    pub stream_type: u8,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiDemuxEvent {
    ProgramFound {
        number: u16,
        pcr_pid: u16,
        streams: Vec<FfiStream>,
    },
    AccessUnit {
        pid: u16,
        codec: FfiCodec,
        pts: Option<u64>,
        dts: Option<u64>,
        random_access: bool,
        data: Vec<u8>,
    },
    Pcr {
        pid: u16,
        pcr: u64,
    },
    Discontinuity {
        pid: u16,
    },
    SyncLost {
        bytes: u64,
    },
    CrcError {
        pid: u16,
    },
}

impl From<ts::Event> for FfiDemuxEvent {
    fn from(e: ts::Event) -> Self {
        match e {
            ts::Event::ProgramFound(p) => FfiDemuxEvent::ProgramFound {
                number: p.number,
                pcr_pid: p.pcr_pid,
                streams: p
                    .streams
                    .into_iter()
                    .map(|s| FfiStream {
                        pid: s.pid,
                        codec: s.codec.into(),
                        stream_type: s.stream_type,
                        language: s.language,
                    })
                    .collect(),
            },
            ts::Event::AccessUnit(a) => FfiDemuxEvent::AccessUnit {
                pid: a.pid,
                codec: a.codec.into(),
                pts: a.pts,
                dts: a.dts,
                random_access: a.random_access,
                data: a.data,
            },
            ts::Event::Pcr { pid, pcr } => FfiDemuxEvent::Pcr { pid, pcr },
            ts::Event::Discontinuity { pid } => FfiDemuxEvent::Discontinuity { pid },
            ts::Event::SyncLost { bytes } => FfiDemuxEvent::SyncLost {
                bytes: bytes as u64,
            },
            ts::Event::CrcError { pid } => FfiDemuxEvent::CrcError { pid },
        }
    }
}

/// Incremental MPEG-TS demuxer. Feed it the bytes of an HTTP `.ts` stream as
/// they arrive; it returns decoder-ready access units.
#[derive(Debug, Default, uniffi::Object)]
pub struct TransportStreamDemuxer {
    inner: Mutex<ts::Demuxer>,
}

#[uniffi::export]
impl TransportStreamDemuxer {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Push a chunk of bytes (any size) and receive the events it completed.
    pub fn push(&self, bytes: Vec<u8>) -> Vec<FfiDemuxEvent> {
        let mut d = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        d.push(&bytes);
        d.events().into_iter().map(FfiDemuxEvent::from).collect()
    }

    /// Emit whatever is buffered (end of stream).
    pub fn flush(&self) -> Vec<FfiDemuxEvent> {
        let mut d = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        d.flush();
        d.events().into_iter().map(FfiDemuxEvent::from).collect()
    }

    /// Forget partial packets and timing after a seek/reconnect. Program
    /// information is kept.
    pub fn reset_timing(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reset_timing();
    }

    pub fn packets_seen(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .packets_seen()
    }
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

/// Version of the core library, for the About screen and bug reports.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_roundtrip_through_ffi_types() {
        let p = parse_playlist(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" group-title=\"G\",Name\nhttp://x/1.ts\n".into(),
        )
        .unwrap();
        assert_eq!(p.channels.len(), 1);
        let cat = ChannelCatalog::new();
        cat.add_channels(p.channels.clone());
        assert_eq!(cat.count(), 1);
        cat.set_favourite(p.channels[0].id.clone(), true);
        let json = cat.to_json();
        let back = ChannelCatalog::from_json(json).unwrap();
        assert!(back.is_favourite(p.channels[0].id.clone()));
        assert_eq!(back.groups(), vec!["G"]);
        assert_eq!(back.search("nam".into(), 5)[0].name, "Name");
    }

    #[test]
    fn epg_guide_load_and_query() {
        let g = EpgGuide::new();
        let doc = r#"<tv><channel id="c"><display-name>Chan</display-name></channel>
<programme start="19700101000000 +0000" stop="19700101010000 +0000" channel="c"><title>T</title></programme></tv>"#;
        assert_eq!(g.load_xmltv(doc.as_bytes().to_vec()).unwrap(), 1);
        let nn = g.now_next("c".into(), 10);
        assert_eq!(nn.now.unwrap().title, "T");
        assert!(nn.next.is_none());
        assert_eq!(g.resolve(None, "chan".into()).as_deref(), Some("c"));
    }

    #[test]
    fn demuxer_object_is_usable() {
        let d = TransportStreamDemuxer::new();
        let events = d.push(vec![0x00, 0x01, 0x02]);
        assert_eq!(events, vec![FfiDemuxEvent::SyncLost { bytes: 3 }]);
        assert!(d.flush().is_empty());
        assert_eq!(d.packets_seen(), 0);
    }

    #[test]
    fn xtream_helpers() {
        let e = XtreamEndpoints::new("host:8080".into(), "u".into(), "p".into()).unwrap();
        assert_eq!(
            e.live_stream_url("1".into(), false),
            "http://host:8080/live/u/p/1.ts"
        );
        let channels = xtream_live_channels(
            &e,
            r#"[{"category_id":"1","category_name":"News"}]"#.into(),
            r#"[{"stream_id":7,"name":"N","category_id":"1"}]"#.into(),
            true,
        )
        .unwrap();
        assert_eq!(channels[0].url, "http://host:8080/live/u/p/7.m3u8");
        assert_eq!(channels[0].group.as_deref(), Some("News"));
        let acct =
            xtream_decode_account(r#"{"user_info":{"status":"Active","auth":1}}"#.into()).unwrap();
        assert!(acct.is_active);
        let eps = xtream_decode_series_info(
            &e,
            r#"{"episodes":{"1":[{"id":"55","episode_num":1,"title":"Pilot","container_extension":"mkv"}]}}"#.into(),
        )
        .unwrap();
        assert_eq!(eps[0].url, "http://host:8080/series/u/p/55.mkv");
        assert_eq!(eps[0].season, 1);
    }
}
