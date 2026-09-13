//! Pulling the first seconds of a stream and reporting what the demuxer sees.
//!
//! This is the part of the desktop shell that tests Streamium rather than the
//! player it hands off to: it runs [`streamium_mpegts::Demuxer`] over real
//! bytes from the user's own provider, on the user's own connection, and
//! reports the programs, codecs, timing and errors found. A tester can copy
//! the result and send it back, which is worth more than "it didn't work".

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use streamium_mpegts::{Codec, Demuxer, Event};

use crate::net;
use streamium_core::model::HttpHints;

/// How much of the stream to pull before reporting, unless stopped earlier.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SECONDS: u64 = 12;
const CHUNK: usize = 64 * 1024;
/// How far to follow HLS manifests to reach an actual segment.
const MANIFEST_DEPTH: usize = 2;

#[derive(Debug, Clone)]
pub struct StreamStat {
    pub pid: u16,
    pub codec: Codec,
    pub stream_type: u8,
    pub language: Option<String>,
    pub units: u64,
    pub bytes: u64,
    pub keyframes: u64,
    pub first_pts: Option<u64>,
    pub last_pts: Option<u64>,
    pub missing_pts: u64,
}

impl StreamStat {
    /// Span covered by the presentation timestamps, in milliseconds.
    pub fn pts_span_ms(&self) -> Option<u64> {
        match (self.first_pts, self.last_pts) {
            (Some(a), Some(b)) if b >= a => Some((b - a) / 90),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProgramInfo {
    pub number: u16,
    pub pmt_pid: u16,
    pub pcr_pid: u16,
    pub stream_pids: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub channel: String,
    pub url: String,
    /// The URL actually analysed, which differs from `url` when an HLS
    /// manifest was followed to one of its segments.
    pub analysed_url: String,
    pub content_type: Option<String>,
    pub followed_manifest: bool,
    pub ttfb_ms: u128,
    pub elapsed_ms: u128,
    pub bytes: u64,
    pub packets: u64,
    pub programs: Vec<ProgramInfo>,
    pub streams: Vec<StreamStat>,
    pub pcr_samples: u64,
    pub discontinuities: u64,
    pub sync_lost_bytes: u64,
    pub crc_errors: u64,
    /// Time from opening the connection to the first video key frame: what a
    /// channel change would cost with a decoder attached.
    pub first_keyframe_ms: Option<u128>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Video and audio were found and the demuxer stayed in sync.
    Good,
    /// It demuxed, but something is off: no audio, no key frame, errors.
    Partial,
    /// Nothing that looks like a transport stream came back.
    NotTransportStream,
}

impl Verdict {
    pub fn headline(self) -> &'static str {
        match self {
            Verdict::Good => "Stream demuxed cleanly",
            Verdict::Partial => "Stream demuxed with problems",
            Verdict::NotTransportStream => "Not a transport stream",
        }
    }
}

impl Report {
    pub fn bitrate_mbps(&self) -> f64 {
        if self.elapsed_ms == 0 {
            return 0.0;
        }
        (self.bytes as f64 * 8.0) / (self.elapsed_ms as f64 * 1000.0)
    }

    pub fn video(&self) -> Option<&StreamStat> {
        self.streams.iter().find(|s| s.codec.is_video())
    }

    pub fn audio(&self) -> Option<&StreamStat> {
        self.streams.iter().find(|s| s.codec.is_audio())
    }

    /// A plain-text version to put on the clipboard.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Streamium stream report");
        let _ = writeln!(out, "  channel:      {}", self.channel);
        let _ = writeln!(out, "  url:          {}", redact(&self.url));
        if self.followed_manifest {
            let _ = writeln!(out, "  segment:      {}", redact(&self.analysed_url));
        }
        if let Some(ct) = &self.content_type {
            let _ = writeln!(out, "  content-type: {ct}");
        }
        let _ = writeln!(out, "  verdict:      {}", self.verdict.headline());
        let _ = writeln!(
            out,
            "  first byte:   {} ms, then {} bytes in {} ms ({:.2} Mbit/s)",
            self.ttfb_ms,
            self.bytes,
            self.elapsed_ms,
            self.bitrate_mbps()
        );
        let _ = writeln!(
            out,
            "  packets:      {} ({} programs, {} PCR samples)",
            self.packets,
            self.programs.len(),
            self.pcr_samples
        );
        match self.first_keyframe_ms {
            Some(ms) => {
                let _ = writeln!(out, "  first key frame at {ms} ms");
            }
            None => {
                let _ = writeln!(out, "  first key frame: none seen");
            }
        }
        let _ = writeln!(
            out,
            "  errors:       {} discontinuities, {} bytes of lost sync, {} CRC failures",
            self.discontinuities, self.sync_lost_bytes, self.crc_errors
        );
        for program in &self.programs {
            let pids = program
                .stream_pids
                .iter()
                .map(|p| format!("0x{p:04x}"))
                .collect::<Vec<_>>()
                .join(" ");
            let _ = writeln!(
                out,
                "  program {} (PMT 0x{:04x}, PCR 0x{:04x}) streams: {pids}",
                program.number, program.pmt_pid, program.pcr_pid
            );
        }
        for s in &self.streams {
            let language = s.language.as_deref().unwrap_or("--");
            let span = s
                .pts_span_ms()
                .map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "no PTS".to_string());
            let _ = writeln!(
                out,
                "  pid 0x{:04x} {:<10} type 0x{:02x} lang {language:<4} {} units, {} bytes, \
                 {} key frames, span {span}",
                s.pid,
                codec_name(s.codec),
                s.stream_type,
                s.units,
                s.bytes,
                s.keyframes
            );
        }
        out
    }
}

/// Replace the credentials in an Xtream URL so a report can be shared.
pub fn redact(url: &str) -> String {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (url, None),
    };
    // Xtream stream URLs carry the account in the path: /live/<user>/<pass>/<id>.ts
    let mut segments: Vec<String> = path.split('/').map(str::to_string).collect();
    if let Some(i) = segments
        .iter()
        .position(|p| matches!(p.as_str(), "live" | "movie" | "series" | "timeshift"))
    {
        for slot in segments.iter_mut().skip(i + 1).take(2) {
            *slot = "***".to_string();
        }
    }
    let mut out = segments.join("/");
    if let Some(q) = query {
        let scrubbed: Vec<String> = q
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((key, _)) if matches!(key, "username" | "password") => format!("{key}=***"),
                _ => pair.to_string(),
            })
            .collect();
        out.push('?');
        out.push_str(&scrubbed.join("&"));
    }
    out
}

pub fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "H.264",
        Codec::H265 => "HEVC",
        Codec::Mpeg2Video => "MPEG-2",
        Codec::AacAdts => "AAC",
        Codec::AacLatm => "AAC LATM",
        Codec::MpegAudio => "MP2",
        Codec::Ac3 => "AC-3",
        Codec::Eac3 => "E-AC-3",
        Codec::Dts => "DTS",
        Codec::DvbSubtitle => "DVB subs",
        Codec::Teletext => "Teletext",
        Codec::Other(_) => "other",
    }
}

/// Fetch and demux the head of a stream. Returns a report, or the reason no
/// report could be made.
pub fn run(
    channel_name: &str,
    url: &str,
    hints: &HttpHints,
    user_agent: &str,
    stop: &Arc<AtomicBool>,
) -> Result<Report, String> {
    let agent = net::media_agent(user_agent);
    let started = Instant::now();

    let mut target = url.to_string();
    let mut followed = false;
    let mut content_type;
    let mut reader;

    // Follow HLS manifests down to a segment: with `prefer_hls` on, the
    // channel URL is a playlist, and there is nothing to demux in that.
    let mut depth = 0;
    loop {
        let (r, ct) =
            net::open_stream(&agent, &target, hints).map_err(|e| format!("Could not open: {e}"))?;
        content_type = ct;
        reader = r;
        // One transport-stream packet is enough to tell a manifest from a
        // stream, and enough for the demuxer to start on.
        let mut head = vec![0u8; 8192];
        let mut filled = 0;
        while filled < 188 {
            match reader.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(format!("Could not read: {e}")),
            }
        }
        head.truncate(filled);
        if filled == 0 {
            return Err("the server closed the connection without sending anything.".to_string());
        }
        let looks_like_manifest = head.starts_with(b"#EXTM3U");
        if !looks_like_manifest || depth >= MANIFEST_DEPTH {
            // Not a manifest: keep what we read and carry on demuxing it.
            return Ok(demux(
                channel_name,
                url,
                &target,
                followed,
                content_type,
                head,
                reader,
                started,
                stop,
            ));
        }
        // It is a manifest. Read it fully and pick the first URI it lists.
        let mut text = String::from_utf8_lossy(&head).to_string();
        let mut rest = Vec::new();
        reader
            .take(4 * 1024 * 1024)
            .read_to_end(&mut rest)
            .map_err(|e| format!("Could not read the playlist: {e}"))?;
        text.push_str(&String::from_utf8_lossy(&rest));
        let next = first_uri(&text)
            .ok_or_else(|| "the playlist listed no streams or segments.".to_string())?;
        target = resolve(&target, &next);
        followed = true;
        depth += 1;
    }
}

/// First non-comment line of an HLS playlist: a variant or a segment.
fn first_uri(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|l| l.to_string())
}

/// Resolve a possibly relative playlist entry against the playlist's URL.
fn resolve(base: &str, reference: &str) -> String {
    if reference.contains("://") {
        return reference.to_string();
    }
    match url::Url::parse(base).and_then(|b| b.join(reference)) {
        Ok(u) => u.to_string(),
        Err(_) => reference.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn demux(
    channel_name: &str,
    original_url: &str,
    analysed_url: &str,
    followed_manifest: bool,
    content_type: Option<String>,
    head: Vec<u8>,
    mut reader: Box<dyn Read + Send>,
    started: Instant,
    stop: &Arc<AtomicBool>,
) -> Report {
    let ttfb_ms = started.elapsed().as_millis();
    let deadline = Duration::from_secs(MAX_SECONDS);

    let mut demuxer = Demuxer::new();
    let mut stats: BTreeMap<u16, StreamStat> = BTreeMap::new();
    let mut programs: BTreeMap<u16, ProgramInfo> = BTreeMap::new();
    let mut pcr_samples = 0u64;
    let mut discontinuities = 0u64;
    let mut sync_lost_bytes = 0u64;
    let mut crc_errors = 0u64;
    let mut first_keyframe_ms = None;
    let mut bytes = head.len() as u64;

    let mut chunk = vec![0u8; CHUNK];
    let mut buffer = head;
    loop {
        demuxer.push(&buffer);
        for event in demuxer.events() {
            match event {
                Event::ProgramFound(p) => {
                    for s in &p.streams {
                        let entry = stats.entry(s.pid).or_insert_with(|| StreamStat {
                            pid: s.pid,
                            codec: s.codec,
                            stream_type: s.stream_type,
                            language: s.language.clone(),
                            units: 0,
                            bytes: 0,
                            keyframes: 0,
                            first_pts: None,
                            last_pts: None,
                            missing_pts: 0,
                        });
                        entry.codec = s.codec;
                        entry.stream_type = s.stream_type;
                        entry.language = s.language.clone();
                    }
                    programs.insert(
                        p.number,
                        ProgramInfo {
                            number: p.number,
                            pmt_pid: p.pmt_pid,
                            pcr_pid: p.pcr_pid,
                            stream_pids: p.streams.iter().map(|s| s.pid).collect(),
                        },
                    );
                }
                Event::AccessUnit(au) => {
                    let entry = stats.entry(au.pid).or_insert_with(|| StreamStat {
                        pid: au.pid,
                        codec: au.codec,
                        stream_type: 0,
                        language: None,
                        units: 0,
                        bytes: 0,
                        keyframes: 0,
                        first_pts: None,
                        last_pts: None,
                        missing_pts: 0,
                    });
                    entry.units += 1;
                    entry.bytes += au.data.len() as u64;
                    match au.pts {
                        Some(pts) => {
                            entry.first_pts.get_or_insert(pts);
                            entry.last_pts = Some(pts);
                        }
                        None => entry.missing_pts += 1,
                    }
                    if au.random_access {
                        entry.keyframes += 1;
                        if au.codec.is_video() && first_keyframe_ms.is_none() {
                            first_keyframe_ms = Some(started.elapsed().as_millis());
                        }
                    }
                }
                Event::Pcr { .. } => pcr_samples += 1,
                Event::Discontinuity { .. } => discontinuities += 1,
                Event::SyncLost { bytes } => sync_lost_bytes += bytes as u64,
                Event::CrcError { .. } => crc_errors += 1,
            }
        }

        if stop.load(Ordering::Relaxed) || bytes >= MAX_BYTES || started.elapsed() >= deadline {
            break;
        }
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                bytes += n as u64;
                buffer.clear();
                buffer.extend_from_slice(&chunk[..n]);
            }
            Err(_) => break,
        }
    }
    demuxer.flush();
    let packets = demuxer.packets_seen();

    let streams: Vec<StreamStat> = stats.into_values().collect();
    let has_video = streams.iter().any(|s| s.codec.is_video() && s.units > 0);
    let has_audio = streams.iter().any(|s| s.codec.is_audio() && s.units > 0);
    let verdict = if programs.is_empty() && packets == 0 {
        Verdict::NotTransportStream
    } else if has_video && has_audio && first_keyframe_ms.is_some() && sync_lost_bytes == 0 {
        Verdict::Good
    } else {
        Verdict::Partial
    };

    Report {
        channel: channel_name.to_string(),
        url: original_url.to_string(),
        analysed_url: analysed_url.to_string(),
        content_type,
        followed_manifest,
        ttfb_ms,
        elapsed_ms: started.elapsed().as_millis(),
        bytes,
        packets,
        programs: programs.into_values().collect(),
        streams,
        pcr_samples,
        discontinuities,
        sync_lost_bytes,
        crc_errors,
        first_keyframe_ms,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_xtream_path_credentials() {
        let out = redact("http://example.com:8080/live/bob/hunter2/1234.ts");
        assert_eq!(out, "http://example.com:8080/live/***/***/1234.ts");
    }

    #[test]
    fn redacts_query_credentials() {
        let out = redact("http://example.com/player_api.php?username=bob&password=hunter2&x=1");
        assert!(out.contains("username=***"), "{out}");
        assert!(out.contains("password=***"), "{out}");
        assert!(out.ends_with("&x=1"), "{out}");
    }

    #[test]
    fn resolves_relative_segments() {
        let out = resolve("http://host/a/b/index.m3u8", "seg1.ts");
        assert_eq!(out, "http://host/a/b/seg1.ts");
    }

    #[test]
    fn first_uri_skips_comments() {
        let text = "#EXTM3U\n#EXT-X-VERSION:3\n\n#EXTINF:6,\nseg.ts\n";
        assert_eq!(first_uri(text).as_deref(), Some("seg.ts"));
    }
}
