//! Tolerant parser for extended M3U playlists (`#EXTM3U` / `#EXTINF`).
//!
//! Real-world IPTV playlists are wildly inconsistent: missing headers, BOMs,
//! Windows line endings, commas inside quoted attributes, attributes without
//! quotes, `#EXTVLCOPT` / `#KODIPROP` / `#EXTHTTP` option lines, and titles
//! containing commas. This parser never fails on a malformed entry; it skips
//! what it cannot understand and keeps going, because a 20 000 channel
//! playlist with one broken line must still load.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::{Catchup, Channel, HttpHints, MediaKind, StreamFormat};

/// The result of parsing a playlist document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Playlist {
    pub kind: PlaylistKind,
    pub channels: Vec<Channel>,
    /// EPG URLs advertised in the header (`url-tvg` / `x-tvg-url`), possibly several, comma separated.
    pub epg_urls: Vec<String>,
    /// Lines the parser did not understand, with their 1-based line numbers. Useful for diagnostics.
    pub warnings: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaylistKind {
    /// A list of channels/items to browse.
    #[default]
    Channels,
    /// An HLS media or master playlist (segments or variants). Play it directly.
    HlsMedia,
}

/// Parse an M3U document. Returns [`CoreError::NotAPlaylist`] only when the
/// text has neither an `#EXTM3U` header nor any `#EXTINF` entry.
pub fn parse(text: &str) -> Result<Playlist, CoreError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines = text.lines().enumerate();

    let mut playlist = Playlist::default();
    let mut saw_header = false;
    let mut saw_extinf = false;

    // State carried from directive lines to the URL line that follows them.
    let mut pending: Option<PendingEntry> = None;
    let mut pending_group: Option<String> = None; // #EXTGRP applies to the next entry only
    let mut pending_http = HttpHints::default();

    for (idx, raw) in lines {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#EXTM3U") {
            saw_header = true;
            let attrs = parse_attributes(rest).0;
            for key in ["url-tvg", "x-tvg-url", "tvg-url"] {
                if let Some(v) = attrs.get(key) {
                    playlist.epg_urls.extend(
                        v.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                    );
                }
            }
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#EXTINF:") {
            saw_extinf = true;
            let (duration, attrs, title) = parse_extinf(rest);
            pending = Some(PendingEntry {
                line_no,
                duration,
                attrs,
                title,
            });
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#EXTGRP:") {
            let g = rest.trim();
            if !g.is_empty() {
                pending_group = Some(g.to_string());
            }
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#EXTVLCOPT:") {
            apply_vlc_option(rest.trim(), &mut pending_http);
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#EXTHTTP:") {
            apply_exthttp(rest.trim(), &mut pending_http);
            continue;
        }

        if let Some(rest) = strip_prefix_ci(line, "#KODIPROP:") {
            apply_kodiprop(rest.trim(), &mut pending_http);
            continue;
        }

        if strip_prefix_ci(line, "#EXT-X-STREAM-INF").is_some()
            || strip_prefix_ci(line, "#EXT-X-TARGETDURATION").is_some()
            || strip_prefix_ci(line, "#EXT-X-MEDIA-SEQUENCE").is_some()
            || strip_prefix_ci(line, "#EXT-X-PLAYLIST-TYPE").is_some()
        {
            // This is a media/master playlist for one stream, not a channel list.
            playlist.kind = PlaylistKind::HlsMedia;
            playlist.channels.clear();
            return Ok(playlist);
        }

        if line.starts_with('#') {
            // Unknown directive or comment. Only report directives; plain comments are common.
            if line.starts_with("#EXT") {
                playlist.warnings.push((
                    line_no,
                    format!("unknown directive: {}", truncate(line, 60)),
                ));
            }
            continue;
        }

        // Anything else is a URL/path for the pending entry.
        let url = line.to_string();
        let entry = pending.take();
        let channel = build_channel(
            &url,
            entry,
            pending_group.take(),
            std::mem::take(&mut pending_http),
            playlist.channels.len(),
        );
        playlist.channels.push(channel);
    }

    if !saw_header && !saw_extinf {
        return Err(CoreError::NotAPlaylist);
    }
    Ok(playlist)
}

struct PendingEntry {
    #[allow(dead_code)]
    line_no: usize,
    #[allow(dead_code)]
    duration: f64,
    attrs: HashMap<String, String>,
    title: String,
}

fn build_channel(
    url: &str,
    entry: Option<PendingEntry>,
    extgrp: Option<String>,
    http: HttpHints,
    index: usize,
) -> Channel {
    let (attrs, title) = match entry {
        Some(e) => (e.attrs, e.title),
        None => (HashMap::new(), String::new()),
    };

    let name = if !title.trim().is_empty() {
        title.trim().to_string()
    } else if let Some(n) = attrs.get("tvg-name").filter(|s| !s.trim().is_empty()) {
        n.trim().to_string()
    } else {
        name_from_url(url)
    };

    let group = attrs
        .get("group-title")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(extgrp);

    let logo = attrs
        .get("tvg-logo")
        .or_else(|| attrs.get("logo"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let epg_id = attrs
        .get("tvg-id")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let number = attrs
        .get("tvg-chno")
        .or_else(|| attrs.get("channel-number"))
        .and_then(|s| s.trim().parse::<u32>().ok());

    let catchup = attrs
        .get("catchup")
        .filter(|k| !k.trim().is_empty())
        .map(|k| Catchup {
            kind: k.trim().to_string(),
            source: attrs.get("catchup-source").map(|s| s.trim().to_string()),
            days: attrs
                .get("catchup-days")
                .and_then(|d| d.trim().parse().ok()),
        });

    let kind = infer_kind(url, &attrs, group.as_deref());

    // A stable id that survives re-ordering of the playlist between refreshes,
    // so favourites and recents keep pointing at the same channel.
    let id = format!("{:016x}", fnv1a64(url.as_bytes(), name.as_bytes()));
    let _ = index;

    let format = StreamFormat::infer(url);
    Channel {
        id,
        name,
        url: url.to_string(),
        kind,
        format,
        group,
        logo,
        epg_id,
        number,
        catchup,
        http,
    }
}

fn infer_kind(url: &str, attrs: &HashMap<String, String>, group: Option<&str>) -> MediaKind {
    if attrs
        .get("radio")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return MediaKind::Radio;
    }
    let lower = url.to_ascii_lowercase();
    if lower.contains("/movie/") {
        return MediaKind::Movie;
    }
    if lower.contains("/series/") {
        return MediaKind::Episode;
    }
    if let Some(g) = group {
        let g = g.to_ascii_lowercase();
        if g.contains("series") {
            return MediaKind::Episode;
        }
        if g.contains("vod") || g.contains("movie") || g.contains("film") {
            return MediaKind::Movie;
        }
        if g.contains("radio") {
            return MediaKind::Radio;
        }
    }
    MediaKind::Live
}

/// Parse the body of an `#EXTINF:` line: `<duration> [key=value ...],<title>`.
fn parse_extinf(rest: &str) -> (f64, HashMap<String, String>, String) {
    let rest = rest.trim_start();
    // Duration ends at the first whitespace or comma.
    let dur_end = rest
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(rest.len());
    let duration = rest[..dur_end].trim().parse::<f64>().unwrap_or(-1.0);
    let after = &rest[dur_end..];
    let (attrs, title) = parse_attributes(after);
    (duration, attrs, title)
}

/// Scan `key=value` / `key="value"` pairs. Stops at the first comma that is
/// outside quotes and not part of a value; the remainder is returned as the
/// title. Keys are lower-cased.
fn parse_attributes(input: &str) -> (HashMap<String, String>, String) {
    let mut attrs = HashMap::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let n = bytes.len();

    loop {
        // Skip whitespace.
        while i < n && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= n {
            return (attrs, String::new());
        }
        if bytes[i] == b',' {
            return (attrs, input[i + 1..].trim().to_string());
        }
        // Read key up to '='.
        let key_start = i;
        while i < n && bytes[i] != b'=' && bytes[i] != b',' && !(bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= n || bytes[i] != b'=' {
            // Not an attribute: treat the rest as the title (there is no comma
            // separator in this malformed line).
            return (attrs, input[key_start..].trim().to_string());
        }
        let key = input[key_start..i].to_ascii_lowercase();
        i += 1; // skip '='
        if i < n && bytes[i] == b'"' {
            i += 1;
            let val_start = i;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            let value = &input[val_start..i.min(n)];
            attrs.insert(key, value.to_string());
            if i < n {
                i += 1; // closing quote
            }
        } else {
            let val_start = i;
            while i < n && bytes[i] != b',' && !(bytes[i] as char).is_whitespace() {
                i += 1;
            }
            attrs.insert(key, input[val_start..i].to_string());
        }
    }
}

fn apply_vlc_option(opt: &str, http: &mut HttpHints) {
    let Some((k, v)) = opt.split_once('=') else {
        return;
    };
    let v = v.trim();
    match k.trim().to_ascii_lowercase().as_str() {
        "http-user-agent" => http.user_agent = Some(v.to_string()),
        "http-referrer" | "http-referer" => http.referrer = Some(v.to_string()),
        "http-origin" => http.headers.push(("Origin".into(), v.to_string())),
        "http-cookie" => http.headers.push(("Cookie".into(), v.to_string())),
        _ => {}
    }
}

fn apply_exthttp(json: &str, http: &mut HttpHints) {
    // `#EXTHTTP:{"User-Agent":"x","Cookie":"y"}`
    if let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(json) {
        for (k, v) in map {
            let Some(v) = v.as_str() else { continue };
            match k.to_ascii_lowercase().as_str() {
                "user-agent" => http.user_agent = Some(v.to_string()),
                "referer" | "referrer" => http.referrer = Some(v.to_string()),
                _ => http.headers.push((k, v.to_string())),
            }
        }
    }
}

fn apply_kodiprop(prop: &str, http: &mut HttpHints) {
    // `#KODIPROP:inputstream.adaptive.stream_headers=User-Agent=x&Referer=y`
    let Some((k, v)) = prop.split_once('=') else {
        return;
    };
    if k.trim()
        .eq_ignore_ascii_case("inputstream.adaptive.stream_headers")
        || k.trim()
            .eq_ignore_ascii_case("inputstream.adaptive.manifest_headers")
    {
        for pair in v.split('&') {
            if let Some((hk, hv)) = pair.split_once('=') {
                match hk.trim().to_ascii_lowercase().as_str() {
                    "user-agent" => http.user_agent = Some(hv.trim().to_string()),
                    "referer" | "referrer" => http.referrer = Some(hv.trim().to_string()),
                    _ => http
                        .headers
                        .push((hk.trim().to_string(), hv.trim().to_string())),
                }
            }
        }
    }
}

fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let seg = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let seg = seg.rsplit_once('.').map(|(s, _)| s).unwrap_or(seg);
    if seg.is_empty() {
        url.to_string()
    } else {
        seg.to_string()
    }
}

fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&line[prefix.len()..])
    } else {
        None
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn fnv1a64(a: &[u8], b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in a.iter().chain(std::iter::once(&0u8)).chain(b.iter()) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}#EXTM3U url-tvg=\"http://epg.example/guide.xml.gz\"\r\n\
#EXTINF:-1 tvg-id=\"bbc1.uk\" tvg-name=\"BBC One\" tvg-logo=\"http://l/bbc1.png\" group-title=\"UK, News\" tvg-chno=\"1\",BBC One HD\r\n\
http://host/live/user/pass/101.ts\r\n\
\r\n\
#EXTINF:-1 group-title=\"Movies\" catchup=\"default\" catchup-days=\"7\",Some Film, The\r\n\
#EXTVLCOPT:http-user-agent=Mozilla/5.0 Custom\r\n\
#EXTVLCOPT:http-referrer=http://ref.example/\r\n\
http://host/movie/user/pass/555.mkv\r\n\
#EXTGRP:Radio Stations\r\n\
#EXTINF:0 radio=\"true\",Jazz FM\r\n\
http://radio.example/jazz.aac\r\n\
#EXTINF:-1,Plain Entry\r\n\
http://host/plain.m3u8\r\n";

    #[test]
    fn parses_extended_playlist() {
        let p = parse(SAMPLE).unwrap();
        assert_eq!(p.kind, PlaylistKind::Channels);
        assert_eq!(p.epg_urls, vec!["http://epg.example/guide.xml.gz"]);
        assert_eq!(p.channels.len(), 4);

        let c = &p.channels[0];
        assert_eq!(c.name, "BBC One HD");
        assert_eq!(c.epg_id.as_deref(), Some("bbc1.uk"));
        assert_eq!(c.group.as_deref(), Some("UK, News"));
        assert_eq!(c.logo.as_deref(), Some("http://l/bbc1.png"));
        assert_eq!(c.number, Some(1));
        assert_eq!(c.kind, MediaKind::Live);
        assert_eq!(c.format, StreamFormat::MpegTs);

        let m = &p.channels[1];
        assert_eq!(m.name, "Some Film, The");
        assert_eq!(m.kind, MediaKind::Movie);
        assert_eq!(m.format, StreamFormat::Progressive);
        assert_eq!(m.catchup.as_ref().unwrap().kind, "default");
        assert_eq!(m.catchup.as_ref().unwrap().days, Some(7));
        assert_eq!(m.http.user_agent.as_deref(), Some("Mozilla/5.0 Custom"));
        assert_eq!(m.http.referrer.as_deref(), Some("http://ref.example/"));

        let r = &p.channels[2];
        assert_eq!(r.name, "Jazz FM");
        assert_eq!(r.kind, MediaKind::Radio);
        assert_eq!(r.group.as_deref(), Some("Radio Stations"));
        // HTTP hints must not leak from the previous entry.
        assert_eq!(r.http, HttpHints::default());

        let plain = &p.channels[3];
        assert_eq!(plain.name, "Plain Entry");
        assert_eq!(plain.format, StreamFormat::Hls);
        assert!(plain.group.is_none());
    }

    #[test]
    fn stable_ids_survive_reordering() {
        let a = parse("#EXTM3U\n#EXTINF:-1,A\nhttp://x/a\n#EXTINF:-1,B\nhttp://x/b\n").unwrap();
        let b = parse("#EXTM3U\n#EXTINF:-1,B\nhttp://x/b\n#EXTINF:-1,A\nhttp://x/a\n").unwrap();
        assert_eq!(a.channels[0].id, b.channels[1].id);
        assert_eq!(a.channels[1].id, b.channels[0].id);
        assert_ne!(a.channels[0].id, a.channels[1].id);
    }

    #[test]
    fn tolerates_missing_header_and_unquoted_attributes() {
        let text = "#EXTINF:-1 tvg-id=abc group-title=Sports,ESPN\nhttp://x/espn\n";
        let p = parse(text).unwrap();
        assert_eq!(p.channels.len(), 1);
        assert_eq!(p.channels[0].epg_id.as_deref(), Some("abc"));
        assert_eq!(p.channels[0].group.as_deref(), Some("Sports"));
        assert_eq!(p.channels[0].name, "ESPN");
    }

    #[test]
    fn unterminated_quote_does_not_panic() {
        let text = "#EXTM3U\n#EXTINF:-1 tvg-name=\"Broken,Channel\nhttp://x/1\n";
        let p = parse(text).unwrap();
        assert_eq!(p.channels.len(), 1);
        assert_eq!(p.channels[0].name, "Broken,Channel");
    }

    #[test]
    fn falls_back_to_url_for_name() {
        let p = parse("#EXTM3U\nhttp://host/path/Channel_7.ts\n").unwrap();
        assert_eq!(p.channels[0].name, "Channel_7");
    }

    #[test]
    fn detects_hls_media_playlist() {
        let text = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg1.ts\n";
        let p = parse(text).unwrap();
        assert_eq!(p.kind, PlaylistKind::HlsMedia);
        assert!(p.channels.is_empty());
    }

    #[test]
    fn rejects_non_playlist() {
        assert_eq!(parse("<html>nope</html>"), Err(CoreError::NotAPlaylist));
        assert_eq!(parse(""), Err(CoreError::NotAPlaylist));
    }

    #[test]
    fn exthttp_and_kodiprop_headers() {
        let text = "#EXTM3U\n#EXTINF:-1,X\n#EXTHTTP:{\"User-Agent\":\"UA1\",\"Cookie\":\"a=b\"}\n#KODIPROP:inputstream.adaptive.stream_headers=Referer=http://r/&X-Token=1\nhttp://x/1\n";
        let p = parse(text).unwrap();
        let h = &p.channels[0].http;
        assert_eq!(h.user_agent.as_deref(), Some("UA1"));
        assert_eq!(h.referrer.as_deref(), Some("http://r/"));
        assert!(h
            .headers
            .contains(&("Cookie".to_string(), "a=b".to_string())));
        assert!(h
            .headers
            .contains(&("X-Token".to_string(), "1".to_string())));
    }

    #[test]
    fn multiple_epg_urls_in_header() {
        let p =
            parse("#EXTM3U x-tvg-url=\"http://a/1.xml, http://b/2.xml\"\n#EXTINF:-1,A\nhttp://x\n")
                .unwrap();
        assert_eq!(p.epg_urls, vec!["http://a/1.xml", "http://b/2.xml"]);
    }
}
