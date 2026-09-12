//! Response models for `player_api.php`.
//!
//! The Xtream API has no schema. The same field arrives as `1`, `"1"`,
//! `true`, `null` or is missing entirely depending on the panel software and
//! version. Every field here therefore decodes through a *flexible*
//! deserializer and is optional unless it is structurally required.

use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::error::CoreError;

/// Decode `T` from a JSON string. The JSON may be an object or, for list
/// endpoints, an array. Some panels return `[]` for "no results" even for
/// object endpoints; those decode as `None` via [`decode_optional`].
pub fn decode<T: de::DeserializeOwned>(json: &str) -> Result<T, CoreError> {
    Ok(serde_json::from_str(json)?)
}

/// Like [`decode`], but treats `[]`, `null`, `false` and `""` as "nothing".
pub fn decode_optional<T: de::DeserializeOwned>(json: &str) -> Result<Option<T>, CoreError> {
    let v: Value = serde_json::from_str(json)?;
    match &v {
        Value::Null | Value::Bool(false) => Ok(None),
        Value::Array(a) if a.is_empty() => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        _ => Ok(Some(serde_json::from_value(v)?)),
    }
}

// ---------- flexible field decoders ----------

/// Decodes a string from a JSON string, number, or bool. `null` → `None`.
pub fn flex_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

/// Decodes an integer from a JSON number or numeric string. Anything else → `None`.
pub fn flex_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
        }
        Some(Value::Bool(b)) => Some(b as i64),
        _ => None,
    })
}

/// Decodes a float from a JSON number or numeric string.
pub fn flex_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    })
}

/// Decodes a bool from `true`/`false`, `0`/`1`, `"0"`/`"1"`, `"true"`/`"false"`.
pub fn flex_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Bool(b)) => Some(b),
        Some(Value::Number(n)) => n.as_i64().map(|i| i != 0),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" | "" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// Decodes a list that may arrive as an array, a single string, `null`, or an
/// object with numeric keys (PHP's `json_encode` of a non-sequential array).
pub fn flex_string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Array(items)) => items.into_iter().filter_map(value_to_string).collect(),
        Some(Value::Object(map)) => map.into_values().filter_map(value_to_string).collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s],
        _ => Vec::new(),
    })
}

fn value_to_string(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

// ---------- account ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AccountInfo {
    #[serde(default)]
    pub user_info: UserInfo,
    #[serde(default)]
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UserInfo {
    #[serde(default, deserialize_with = "flex_string")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub exp_date: Option<i64>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub is_trial: Option<bool>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub active_cons: Option<i64>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub max_connections: Option<i64>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub auth: Option<bool>,
    #[serde(default, deserialize_with = "flex_string_list")]
    pub allowed_output_formats: Vec<String>,
}

impl UserInfo {
    /// `true` when the panel reports the account as usable.
    pub fn is_active(&self) -> bool {
        self.auth.unwrap_or(true)
            && self
                .status
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("Active"))
                .unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerInfo {
    #[serde(default, deserialize_with = "flex_string")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub port: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub https_port: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub server_protocol: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub timezone: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub timestamp_now: Option<i64>,
}

// ---------- categories ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Category {
    #[serde(default, deserialize_with = "flex_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub category_name: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub parent_id: Option<i64>,
}

// ---------- live ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LiveStream {
    #[serde(default, deserialize_with = "flex_i64")]
    pub num: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_type: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub epg_channel_id: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub added: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string_list")]
    pub category_ids: Vec<String>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub tv_archive: Option<bool>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub tv_archive_duration: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub direct_source: Option<String>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub is_adult: Option<bool>,
}

// ---------- vod ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VodStream {
    #[serde(default, deserialize_with = "flex_i64")]
    pub num: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_type: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub stream_icon: Option<String>,
    #[serde(default, deserialize_with = "flex_f64")]
    pub rating: Option<f64>,
    #[serde(default, deserialize_with = "flex_f64")]
    pub rating_5based: Option<f64>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub added: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string_list")]
    pub category_ids: Vec<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub container_extension: Option<String>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub is_adult: Option<bool>,
    #[serde(default, deserialize_with = "flex_string")]
    pub plot: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub year: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub genre: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub duration: Option<String>,
}

// ---------- series ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SeriesListing {
    #[serde(default, deserialize_with = "flex_i64")]
    pub num: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub series_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub cover: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub plot: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub cast: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub genre: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub release_date: Option<String>,
    #[serde(default, deserialize_with = "flex_f64")]
    pub rating: Option<f64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub category_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string_list")]
    pub category_ids: Vec<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub last_modified: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Episode {
    #[serde(default, deserialize_with = "flex_string")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub episode_num: Option<i64>,
    #[serde(default, deserialize_with = "flex_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub container_extension: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub season: Option<i64>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub added: Option<i64>,
    #[serde(default)]
    pub info: Option<EpisodeInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EpisodeInfo {
    #[serde(default, deserialize_with = "flex_string")]
    pub plot: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub movie_image: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub duration: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub duration_secs: Option<i64>,
    #[serde(default, deserialize_with = "flex_f64")]
    pub rating: Option<f64>,
}

/// `get_series_info` returns `episodes` either as an object keyed by season
/// number (`{"1": [...], "2": [...]}`) or as an array of arrays. This type
/// accepts both and exposes a flat, sorted list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SeriesInfo {
    #[serde(default)]
    pub info: Option<SeriesListing>,
    #[serde(default, deserialize_with = "flex_episodes")]
    pub episodes: Vec<Episode>,
}

fn flex_episodes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Episode>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    let mut out = Vec::new();
    match v {
        Some(Value::Object(seasons)) => {
            for (season_key, list) in seasons {
                let season_no = season_key.parse::<i64>().ok();
                push_episodes(&mut out, list, season_no);
            }
        }
        Some(Value::Array(seasons)) => {
            for list in seasons {
                push_episodes(&mut out, list, None);
            }
        }
        _ => {}
    }
    out.sort_by_key(|e| (e.season.unwrap_or(0), e.episode_num.unwrap_or(0)));
    Ok(out)
}

fn push_episodes(out: &mut Vec<Episode>, list: Value, season_no: Option<i64>) {
    if let Value::Array(items) = list {
        for item in items {
            if let Ok(mut ep) = serde_json::from_value::<Episode>(item) {
                if ep.season.is_none() {
                    ep.season = season_no;
                }
                out.push(ep);
            }
        }
    }
}

// ---------- EPG ----------

/// One entry from `get_short_epg` / `get_simple_data_table`. Titles and
/// descriptions are base64 encoded by the panel; use [`EpgListing::title_text`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EpgListing {
    #[serde(default, deserialize_with = "flex_string")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub epg_id: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "flex_string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub start_timestamp: Option<i64>,
    #[serde(default, deserialize_with = "flex_i64")]
    pub stop_timestamp: Option<i64>,
    #[serde(default, deserialize_with = "flex_bool")]
    pub has_archive: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EpgResponse {
    #[serde(default)]
    pub epg_listings: Vec<EpgListing>,
}

impl EpgListing {
    pub fn title_text(&self) -> String {
        self.title
            .as_deref()
            .map(decode_base64_lossy)
            .unwrap_or_default()
    }
    pub fn description_text(&self) -> String {
        self.description
            .as_deref()
            .map(decode_base64_lossy)
            .unwrap_or_default()
    }
}

/// Decode standard base64 (with or without padding). If the input is not
/// valid base64 it is returned unchanged, because some panels send plain text.
pub fn decode_base64_lossy(s: &str) -> String {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    if clean.is_empty() {
        return String::new();
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &c in &clean {
        let Some(v) = val(c) else {
            return s.to_string();
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    match String::from_utf8(out) {
        Ok(text) => text,
        Err(_) => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_info_with_mixed_types() {
        let json = r#"{
          "user_info": {"username":"bob","status":"Active","exp_date":"1735689600","is_trial":"0",
                        "active_cons":1,"max_connections":"2","auth":1,
                        "allowed_output_formats":["m3u8","ts"]},
          "server_info": {"url":"tv.example","port":8080,"https_port":"443","timezone":"UTC",
                          "timestamp_now":1700000000}
        }"#;
        let a: AccountInfo = decode(json).unwrap();
        assert_eq!(a.user_info.username.as_deref(), Some("bob"));
        assert_eq!(a.user_info.exp_date, Some(1735689600));
        assert_eq!(a.user_info.is_trial, Some(false));
        assert_eq!(a.user_info.max_connections, Some(2));
        assert_eq!(a.user_info.auth, Some(true));
        assert!(a.user_info.is_active());
        assert_eq!(a.user_info.allowed_output_formats, vec!["m3u8", "ts"]);
        assert_eq!(a.server_info.port.as_deref(), Some("8080"));
    }

    #[test]
    fn live_streams_list() {
        let json = r#"[
          {"num":1,"name":"BBC One","stream_type":"live","stream_id":101,"stream_icon":"",
           "epg_channel_id":"bbc1.uk","added":"1600000000","category_id":"3","category_ids":[3,9],
           "tv_archive":1,"tv_archive_duration":"7","direct_source":"","is_adult":"0"},
          {"num":"2","name":"Weird","stream_id":"102","category_ids":{"0":"4"},"tv_archive":"false"}
        ]"#;
        let list: Vec<LiveStream> = decode(json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].stream_id.as_deref(), Some("101"));
        assert_eq!(list[0].tv_archive, Some(true));
        assert_eq!(list[0].tv_archive_duration, Some(7));
        assert_eq!(list[0].category_ids, vec!["3", "9"]);
        assert_eq!(list[1].num, Some(2));
        assert_eq!(list[1].category_ids, vec!["4"]);
        assert_eq!(list[1].tv_archive, Some(false));
    }

    #[test]
    fn series_info_with_object_seasons() {
        let json = r#"{"info":{"name":"Show","series_id":"9"},
          "episodes":{"2":[{"id":"22","episode_num":"1","title":"S2E1","container_extension":"mkv"}],
                      "1":[{"id":"11","episode_num":2,"title":"S1E2","season":1},
                           {"id":"10","episode_num":1,"title":"S1E1","info":{"duration_secs":"1500"}}]}}"#;
        let s: SeriesInfo = decode(json).unwrap();
        let titles: Vec<_> = s
            .episodes
            .iter()
            .map(|e| e.title.clone().unwrap())
            .collect();
        assert_eq!(titles, vec!["S1E1", "S1E2", "S2E1"]);
        assert_eq!(
            s.episodes[0].info.as_ref().unwrap().duration_secs,
            Some(1500)
        );
        assert_eq!(s.episodes[2].season, Some(2));
    }

    #[test]
    fn empty_array_means_none() {
        let r: Option<AccountInfo> = decode_optional("[]").unwrap();
        assert!(r.is_none());
        let r: Option<Vec<Category>> =
            decode_optional(r#"[{"category_id":1,"category_name":"A"}]"#).unwrap();
        assert_eq!(r.unwrap()[0].category_id.as_deref(), Some("1"));
    }

    #[test]
    fn epg_titles_are_base64() {
        let json = r#"{"epg_listings":[{"id":"1","title":"TmV3cyBhdCBTaXg=","description":"","start_timestamp":"1700000000","stop_timestamp":"1700003600","has_archive":1}]}"#;
        let r: EpgResponse = decode(json).unwrap();
        assert_eq!(r.epg_listings[0].title_text(), "News at Six");
        assert_eq!(r.epg_listings[0].description_text(), "");
        assert_eq!(decode_base64_lossy("Plain title!"), "Plain title!");
        assert_eq!(decode_base64_lossy("aGk"), "hi");
    }
}
