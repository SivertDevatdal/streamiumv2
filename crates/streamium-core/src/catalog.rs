//! Channel catalog: the merged, searchable list of everything a source offers.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::m3u::Playlist;
use crate::model::{Catchup, Channel, MediaKind, StreamFormat};
use crate::xtream::models::{Category, LiveStream, VodStream};
use crate::xtream::{Endpoints, LiveContainer};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    channels: Vec<Channel>,
    /// Group names in order of first appearance.
    groups: Vec<String>,
    favourites: HashSet<String>,
    #[serde(skip)]
    by_id: HashMap<String, usize>,
}

/// A search hit with a relevance score (higher is better).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit<'a> {
    pub channel: &'a Channel,
    pub score: u32,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_playlist(playlist: &Playlist) -> Self {
        let mut c = Catalog::new();
        for ch in &playlist.channels {
            c.push(ch.clone());
        }
        c
    }

    /// Build the live TV portion of a catalog from Xtream listings.
    pub fn from_xtream_live(
        endpoints: &Endpoints,
        categories: &[Category],
        streams: &[LiveStream],
        container: LiveContainer,
    ) -> Self {
        let cat_names: HashMap<&str, &str> = categories
            .iter()
            .filter_map(|c| Some((c.category_id.as_deref()?, c.category_name.as_deref()?)))
            .collect();
        let mut c = Catalog::new();
        for s in streams {
            let Some(id) = s.stream_id.as_deref() else {
                continue;
            };
            let name = s.name.clone().unwrap_or_else(|| format!("Channel {id}"));
            let url = s
                .direct_source
                .clone()
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| endpoints.live_stream_url(id, container));
            let mut ch = Channel::new(format!("xc-live-{id}"), name, url);
            ch.kind = MediaKind::Live;
            ch.format = match container {
                LiveContainer::Ts => StreamFormat::MpegTs,
                LiveContainer::Hls => StreamFormat::Hls,
            };
            ch.group = s
                .category_id
                .as_deref()
                .and_then(|c| cat_names.get(c))
                .map(|n| n.to_string());
            ch.logo = s.stream_icon.clone().filter(|l| !l.is_empty());
            ch.epg_id = s.epg_channel_id.clone().filter(|e| !e.is_empty());
            ch.number = s.num.and_then(|n| u32::try_from(n).ok());
            if s.tv_archive == Some(true) {
                ch.catchup = Some(Catchup {
                    kind: "xc".into(),
                    source: None,
                    days: s.tv_archive_duration.and_then(|d| u32::try_from(d).ok()),
                });
            }
            c.push(ch);
        }
        c
    }

    /// Build the VOD portion of a catalog from Xtream listings.
    pub fn from_xtream_vod(
        endpoints: &Endpoints,
        categories: &[Category],
        streams: &[VodStream],
    ) -> Self {
        let cat_names: HashMap<&str, &str> = categories
            .iter()
            .filter_map(|c| Some((c.category_id.as_deref()?, c.category_name.as_deref()?)))
            .collect();
        let mut c = Catalog::new();
        for s in streams {
            let Some(id) = s.stream_id.as_deref() else {
                continue;
            };
            let name = s
                .name
                .clone()
                .or_else(|| s.title.clone())
                .unwrap_or_else(|| format!("Movie {id}"));
            let ext = s.container_extension.as_deref().unwrap_or("mp4");
            let mut ch = Channel::new(format!("xc-vod-{id}"), name, endpoints.movie_url(id, ext));
            ch.kind = MediaKind::Movie;
            ch.group = s
                .category_id
                .as_deref()
                .and_then(|c| cat_names.get(c))
                .map(|n| n.to_string());
            ch.logo = s.stream_icon.clone().filter(|l| !l.is_empty());
            c.push(ch);
        }
        c
    }

    pub fn push(&mut self, channel: Channel) {
        if let Some(g) = &channel.group {
            if !self.groups.iter().any(|x| x == g) {
                self.groups.push(g.clone());
            }
        }
        self.by_id.insert(channel.id.clone(), self.channels.len());
        self.channels.push(channel);
    }

    /// Append every channel of `other` (used to merge live + VOD, or several sources).
    pub fn extend(&mut self, other: Catalog) {
        for ch in other.channels {
            self.push(ch);
        }
        self.favourites.extend(other.favourites);
    }

    /// Rebuild the id index after deserialisation.
    pub fn reindex(&mut self) {
        self.by_id = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }
    pub fn groups(&self) -> &[String] {
        &self.groups
    }
    pub fn get(&self, id: &str) -> Option<&Channel> {
        self.by_id.get(id).map(|&i| &self.channels[i])
    }
    pub fn in_group<'a>(&'a self, group: &'a str) -> impl Iterator<Item = &'a Channel> + 'a {
        self.channels
            .iter()
            .filter(move |c| c.group.as_deref() == Some(group))
    }
    pub fn of_kind(&self, kind: MediaKind) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(move |c| c.kind == kind)
    }
    pub fn by_number(&self, number: u32) -> Option<&Channel> {
        self.channels.iter().find(|c| c.number == Some(number))
    }

    pub fn is_favourite(&self, id: &str) -> bool {
        self.favourites.contains(id)
    }
    pub fn set_favourite(&mut self, id: &str, on: bool) {
        if on {
            self.favourites.insert(id.to_string());
        } else {
            self.favourites.remove(id);
        }
    }
    pub fn favourites(&self) -> impl Iterator<Item = &Channel> {
        self.channels
            .iter()
            .filter(|c| self.favourites.contains(&c.id))
    }

    /// Rank channels by how well `query` matches their name, group or number.
    /// Multi-word queries require every word to match somewhere.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit<'_>> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let number: Option<u32> = q.parse().ok();

        let mut hits: Vec<SearchHit> = self
            .channels
            .iter()
            .filter_map(|c| {
                let name = c.name.to_lowercase();
                let group = c
                    .group
                    .as_deref()
                    .map(str::to_lowercase)
                    .unwrap_or_default();
                let mut score = 0u32;
                if name == q {
                    score = score.max(1000);
                } else if name.starts_with(&q) {
                    score = score.max(800);
                } else if name.split_whitespace().any(|w| w.starts_with(&q)) {
                    score = score.max(600);
                } else if name.contains(&q) {
                    score = score.max(400);
                }
                if number.is_some() && c.number == number {
                    score = score.max(900);
                }
                if score == 0 && words.iter().all(|w| name.contains(w) || group.contains(w)) {
                    score = if group.contains(&q) { 200 } else { 300 };
                }
                (score > 0).then_some(SearchHit { channel: c, score })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.channel.name.cmp(&b.channel.name))
        });
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalog {
        let text = "#EXTM3U\n\
#EXTINF:-1 tvg-chno=\"1\" group-title=\"News\",BBC News\nhttp://x/1\n\
#EXTINF:-1 tvg-chno=\"2\" group-title=\"News\",Sky News\nhttp://x/2\n\
#EXTINF:-1 group-title=\"Sport\",BT Sport 1\nhttp://x/3\n\
#EXTINF:-1 group-title=\"Kids\",CBBC\nhttp://x/4\n";
        Catalog::from_playlist(&crate::m3u::parse(text).unwrap())
    }

    #[test]
    fn groups_and_lookup() {
        let c = cat();
        assert_eq!(c.groups(), &["News", "Sport", "Kids"]);
        assert_eq!(c.in_group("News").count(), 2);
        assert_eq!(c.by_number(2).unwrap().name, "Sky News");
        let id = c.channels()[0].id.clone();
        assert_eq!(c.get(&id).unwrap().name, "BBC News");
    }

    #[test]
    fn search_ranking() {
        let c = cat();
        let names = |q: &str| {
            c.search(q, 10)
                .iter()
                .map(|h| h.channel.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("bbc news"), vec!["BBC News"]);
        assert_eq!(names("news"), vec!["BBC News", "Sky News"]);
        assert_eq!(names("sport"), vec!["BT Sport 1"]);
        assert_eq!(names("2"), vec!["Sky News"]);
        assert_eq!(names("bbc"), vec!["BBC News", "CBBC"]);
        assert!(names("zzz").is_empty());
        assert!(names("   ").is_empty());
    }

    #[test]
    fn favourites_roundtrip() {
        let mut c = cat();
        let id = c.channels()[3].id.clone();
        c.set_favourite(&id, true);
        assert!(c.is_favourite(&id));
        assert_eq!(c.favourites().count(), 1);
        let json = serde_json::to_string(&c).unwrap();
        let mut back: Catalog = serde_json::from_str(&json).unwrap();
        back.reindex();
        assert!(back.is_favourite(&id));
        assert_eq!(back.get(&id).unwrap().name, "CBBC");
        c.set_favourite(&id, false);
        assert_eq!(c.favourites().count(), 0);
    }

    #[test]
    fn xtream_live_catalog() {
        let e = Endpoints::new("http://h", "u", "p").unwrap();
        let cats = vec![Category {
            category_id: Some("3".into()),
            category_name: Some("UK".into()),
            parent_id: None,
        }];
        let streams = vec![
            LiveStream {
                stream_id: Some("101".into()),
                name: Some("BBC One".into()),
                category_id: Some("3".into()),
                epg_channel_id: Some("bbc1.uk".into()),
                tv_archive: Some(true),
                tv_archive_duration: Some(7),
                num: Some(1),
                ..Default::default()
            },
            LiveStream {
                stream_id: None,
                name: Some("broken".into()),
                ..Default::default()
            },
        ];
        let c = Catalog::from_xtream_live(&e, &cats, &streams, LiveContainer::Ts);
        assert_eq!(c.len(), 1);
        let ch = &c.channels()[0];
        assert_eq!(ch.url, "http://h/live/u/p/101.ts");
        assert_eq!(ch.group.as_deref(), Some("UK"));
        assert_eq!(ch.catchup.as_ref().unwrap().days, Some(7));
        assert_eq!(ch.format, StreamFormat::MpegTs);
    }
}
