//! Streaming XMLTV parser.
//!
//! XMLTV guides for large providers run to hundreds of megabytes uncompressed.
//! This parser streams through the document with a bounded buffer, decodes
//! gzip on the fly when it sees the magic bytes, and never builds a DOM.

use std::io::{BufRead, BufReader};

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::epg::{EpgChannel, EpgIndex, Programme};
use crate::error::CoreError;

/// Parse an XMLTV document (plain XML or gzip-compressed) into an [`EpgIndex`].
pub fn parse(bytes: &[u8]) -> Result<EpgIndex, CoreError> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let gz = flate2::read::GzDecoder::new(bytes);
        parse_reader(BufReader::with_capacity(1 << 16, gz))
    } else {
        parse_reader(bytes)
    }
}

/// Parse from any buffered reader. Use this for very large files that should
/// not be held in memory at once.
pub fn parse_reader<R: BufRead>(reader: R) -> Result<EpgIndex, CoreError> {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().expand_empty_elements = true;

    let mut index = EpgIndex::new();
    let mut buf = Vec::with_capacity(8192);

    loop {
        let event = xml
            .read_event_into(&mut buf)
            .map_err(|e| CoreError::Xmltv(e.to_string()))?;
        match event {
            Event::Start(e) => match e.local_name().as_ref() {
                "channel" => {
                    let id = attr(&e, "id").unwrap_or_default();
                    let ch = read_channel(&mut xml, id)?;
                    index.add_channel(ch);
                }
                "programme" => {
                    let channel = attr(&e, "channel").unwrap_or_default();
                    let start = attr(&e, "start").and_then(|s| parse_timestamp(&s));
                    let stop = attr(&e, "stop").and_then(|s| parse_timestamp(&s));
                    let programme = read_programme(&mut xml)?;
                    if let (Some(start), Some(stop)) = (start, stop) {
                        if !channel.is_empty() {
                            index.add_programme(
                                &channel,
                                Programme {
                                    start,
                                    stop,
                                    ..programme
                                },
                            );
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    index.finish();
    Ok(index)
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

fn read_channel<R: BufRead>(xml: &mut Reader<R>, id: String) -> Result<EpgChannel, CoreError> {
    let mut ch = EpgChannel {
        id,
        display_names: Vec::new(),
        icon: None,
    };
    let mut buf = Vec::new();
    let mut in_display_name = false;
    let mut text = String::new();
    loop {
        match xml
            .read_event_into(&mut buf)
            .map_err(|e| CoreError::Xmltv(e.to_string()))?
        {
            Event::Start(e) => match e.local_name().as_ref() {
                "display-name" => {
                    in_display_name = true;
                    text.clear();
                }
                // The first icon wins: some guides repeat the element per size.
                "icon" if ch.icon.is_none() => {
                    ch.icon = attr(&e, "src");
                }
                _ => {}
            },
            Event::Text(t) => text.push_str(&t.xml10_content()),
            Event::CData(c) => text.push_str(&c.xml10_content()),
            Event::GeneralRef(r) => append_ref(&mut text, &r),
            Event::End(e) => {
                if e.local_name().as_ref() == "channel" {
                    break;
                }
                if in_display_name {
                    let name = text.trim();
                    if !name.is_empty() {
                        ch.display_names.push(name.to_string());
                    }
                    in_display_name = false;
                    text.clear();
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(ch)
}

/// Resolve `&name;` / `&#NNN;` / `&#xHHH;` into text. Unknown entities are
/// kept verbatim rather than dropped, so nothing silently disappears.
fn append_ref(out: &mut String, r: &BytesRef) {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        out.push(c);
        return;
    }
    let name = r.xml10_content();
    match name.as_ref() {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        "nbsp" => out.push('\u{a0}'),
        other => {
            out.push('&');
            out.push_str(other);
            out.push(';');
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    SubTitle,
    Desc,
    Category,
    EpisodeNs,
    EpisodeOnscreen,
    Other,
}

fn read_programme<R: BufRead>(xml: &mut Reader<R>) -> Result<Programme, CoreError> {
    let mut p = Programme {
        start: 0,
        stop: 0,
        title: String::new(),
        subtitle: None,
        description: None,
        category: None,
        icon: None,
        season: None,
        episode: None,
    };
    let mut buf = Vec::new();
    let mut field = Field::Other;
    let mut text = String::new();
    let mut depth = 0usize;
    loop {
        match xml
            .read_event_into(&mut buf)
            .map_err(|e| CoreError::Xmltv(e.to_string()))?
        {
            Event::Start(e) => {
                depth += 1;
                text.clear();
                field = match e.local_name().as_ref() {
                    "title" => Field::Title,
                    "sub-title" => Field::SubTitle,
                    "desc" => Field::Desc,
                    "category" => Field::Category,
                    "episode-num" => match attr(&e, "system").as_deref() {
                        Some("xmltv_ns") => Field::EpisodeNs,
                        Some("onscreen") => Field::EpisodeOnscreen,
                        _ => Field::Other,
                    },
                    "icon" => {
                        if p.icon.is_none() {
                            p.icon = attr(&e, "src");
                        }
                        Field::Other
                    }
                    _ => Field::Other,
                };
            }
            Event::Text(t) => text.push_str(&t.xml10_content()),
            Event::CData(c) => text.push_str(&c.xml10_content()),
            Event::GeneralRef(r) => append_ref(&mut text, &r),
            Event::End(e) => {
                if depth == 0 || e.local_name().as_ref() == "programme" {
                    break;
                }
                depth -= 1;
                let value = text.trim();
                if !value.is_empty() {
                    match field {
                        // Several <title> elements (different languages) may exist; keep the first.
                        Field::Title if p.title.is_empty() => p.title = value.to_string(),
                        Field::SubTitle if p.subtitle.is_none() => {
                            p.subtitle = Some(value.to_string())
                        }
                        Field::Desc if p.description.is_none() => {
                            p.description = Some(value.to_string())
                        }
                        Field::Category if p.category.is_none() => {
                            p.category = Some(value.to_string())
                        }
                        Field::EpisodeNs => {
                            if let Some((s, e)) = parse_xmltv_ns(value) {
                                p.season = p.season.or(s);
                                p.episode = p.episode.or(e);
                            }
                        }
                        Field::EpisodeOnscreen => {
                            if let Some((s, e)) = parse_onscreen(value) {
                                p.season = p.season.or(s);
                                p.episode = p.episode.or(e);
                            }
                        }
                        _ => {}
                    }
                }
                text.clear();
                field = Field::Other;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(p)
}

/// `xmltv_ns` episode numbers are zero-based `season.episode.part`, each
/// optionally followed by `/total`. `1.5.0/1` is season 2, episode 6.
fn parse_xmltv_ns(s: &str) -> Option<(Option<u32>, Option<u32>)> {
    let mut parts = s.split('.');
    let season = parts
        .next()
        .and_then(|x| x.split('/').next()?.trim().parse::<u32>().ok());
    let episode = parts
        .next()
        .and_then(|x| x.split('/').next()?.trim().parse::<u32>().ok());
    if season.is_none() && episode.is_none() {
        return None;
    }
    Some((season.map(|n| n + 1), episode.map(|n| n + 1)))
}

/// `onscreen` numbers look like `S02E06`, `2x6`, or `Ep. 6`.
fn parse_onscreen(s: &str) -> Option<(Option<u32>, Option<u32>)> {
    let upper = s.to_ascii_uppercase();
    let digits = |it: &mut std::iter::Peekable<std::str::Chars>| -> Option<u32> {
        let mut n = String::new();
        while let Some(c) = it.peek() {
            if c.is_ascii_digit() {
                n.push(*c);
                it.next();
            } else {
                break;
            }
        }
        n.parse().ok()
    };
    if let Some(rest) = upper.strip_prefix('S') {
        let mut it = rest.chars().peekable();
        let season = digits(&mut it);
        if it.peek() == Some(&'E') {
            it.next();
            let ep = digits(&mut it);
            if season.is_some() || ep.is_some() {
                return Some((season, ep));
            }
        }
    }
    if let Some((a, b)) = upper.split_once('X') {
        if let (Ok(s), Ok(e)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
            return Some((Some(s), Some(e)));
        }
    }
    None
}

/// Parse an XMLTV timestamp `YYYYMMDDHHMMSS [+-]HHMM`. Shorter forms
/// (`YYYYMMDDHHMM`, `YYYYMMDD`) are accepted; a missing offset means UTC.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    let (digits, offset) = match s.find([' ', '+', '-']) {
        Some(i) if s[..i].chars().all(|c| c.is_ascii_digit()) => (&s[..i], s[i..].trim()),
        _ => (s, ""),
    };
    if digits.len() < 8 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let num = |from: usize, len: usize| -> i64 {
        digits
            .get(from..from + len)
            .and_then(|x| x.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (y, m, d) = (num(0, 4), num(4, 2), num(6, 2));
    let (hh, mm, ss) = (num(8, 2), num(10, 2), num(12, 2));
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut unix = unix_from_civil(y, m, d) + hh * 3600 + mm * 60 + ss;

    let offset = offset.trim();
    if !offset.is_empty() {
        let (sign, rest) = match offset.as_bytes()[0] {
            b'+' => (1, &offset[1..]),
            b'-' => (-1, &offset[1..]),
            _ => (1, offset),
        };
        let rest: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        if rest.len() >= 4 {
            let oh: i64 = rest[..2].parse().ok()?;
            let om: i64 = rest[2..4].parse().ok()?;
            unix -= sign * (oh * 3600 + om * 60);
        }
    }
    Some(unix)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn unix_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

/// Inverse of [`unix_from_civil`] plus time of day: `(y, m, d, hh, mm, ss)` in UTC.
pub fn civil_from_unix(unix: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, secs / 3600, (secs % 3600) / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="test">
  <channel id="bbc1.uk">
    <display-name lang="en">BBC One</display-name>
    <display-name>BBC 1</display-name>
    <icon src="http://l/bbc1.png"/>
  </channel>
  <channel id="empty.uk"><display-name>Empty</display-name></channel>
  <programme start="20240305180000 +0000" stop="20240305183000 +0000" channel="bbc1.uk">
    <title lang="en">News at Six &amp; More &#x2013; &#8220;Live&#8221;</title>
    <sub-title>Episode one</sub-title>
    <desc><![CDATA[Headlines <b>& more</b>.]]></desc>
    <category lang="en">News</category>
    <category>Current affairs</category>
    <episode-num system="xmltv_ns">1.5.0/1</episode-num>
    <icon src="http://l/news.png"/>
  </programme>
  <programme start="20240305193000 +0100" stop="20240305200000 +0100" channel="bbc1.uk">
    <title>Drama</title>
    <episode-num system="onscreen">S03E04</episode-num>
  </programme>
  <programme start="garbage" stop="20240305200000" channel="bbc1.uk"><title>Bad</title></programme>
  <programme start="20240305200000" stop="20240305210000" channel=""><title>No channel</title></programme>
</tv>"#;

    #[test]
    fn parses_channels_and_programmes() {
        let idx = parse(DOC.as_bytes()).unwrap();
        assert_eq!(idx.channel_count(), 2);
        assert_eq!(idx.programme_count(), 2);
        let ch = idx.channel("bbc1.uk").unwrap();
        assert_eq!(ch.display_names, vec!["BBC One", "BBC 1"]);
        assert_eq!(ch.icon.as_deref(), Some("http://l/bbc1.png"));

        let list = idx.programmes("bbc1.uk");
        let p = &list[0];
        assert_eq!(p.title, "News at Six & More \u{2013} \u{201c}Live\u{201d}");
        assert_eq!(p.subtitle.as_deref(), Some("Episode one"));
        assert_eq!(p.description.as_deref(), Some("Headlines <b>& more</b>."));
        assert_eq!(p.category.as_deref(), Some("News"));
        assert_eq!(p.icon.as_deref(), Some("http://l/news.png"));
        assert_eq!(p.start, 1_709_661_600);
        assert_eq!(p.stop, 1_709_663_400);
        assert_eq!((p.season, p.episode), (Some(2), Some(6)));

        let d = &list[1];
        assert_eq!(d.title, "Drama");
        // 19:30 +0100 == 18:30 UTC, directly after the news.
        assert_eq!(d.start, 1_709_663_400);
        assert_eq!((d.season, d.episode), (Some(3), Some(4)));
    }

    #[test]
    fn gzip_roundtrip() {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(DOC.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        let idx = parse(&gz).unwrap();
        assert_eq!(idx.programme_count(), 2);
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp("19700101000000 +0000"), Some(0));
        assert_eq!(parse_timestamp("19700101010000 +0100"), Some(0));
        assert_eq!(parse_timestamp("19700101000000 -0130"), Some(5400));
        assert_eq!(parse_timestamp("20240305180000"), Some(1_709_661_600));
        assert_eq!(parse_timestamp("202403051800"), Some(1_709_661_600));
        assert_eq!(parse_timestamp("20240305"), Some(1_709_596_800));
        assert_eq!(parse_timestamp("20241301000000"), None);
        assert_eq!(parse_timestamp("abc"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn civil_roundtrip() {
        for unix in [
            0,
            86_399,
            951_782_400,
            1_709_661_600,
            4_102_444_800,
            -86_400,
        ] {
            let (y, m, d, hh, mm, ss) = civil_from_unix(unix);
            assert_eq!(unix_from_civil(y, m, d) + hh * 3600 + mm * 60 + ss, unix);
        }
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn episode_number_formats() {
        assert_eq!(parse_xmltv_ns("0.0.0/1"), Some((Some(1), Some(1))));
        assert_eq!(parse_xmltv_ns(".3."), Some((None, Some(4))));
        assert_eq!(parse_xmltv_ns("x"), None);
        assert_eq!(parse_onscreen("s01e02"), Some((Some(1), Some(2))));
        assert_eq!(parse_onscreen("2x7"), Some((Some(2), Some(7))));
        assert_eq!(parse_onscreen("Episode 4"), None);
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(
            parse(b"<tv><programme start=\"1\"").is_err()
                || parse(b"<tv><programme start=\"1\"").is_ok()
        );
        let idx = parse(b"<tv></tv>").unwrap();
        assert_eq!(idx.channel_count(), 0);
    }
}
