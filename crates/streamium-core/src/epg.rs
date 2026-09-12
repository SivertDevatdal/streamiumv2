//! In-memory electronic programme guide.
//!
//! Programmes are stored per channel, sorted by start time, so "what's on
//! now", "what's next" and "everything between 18:00 and 22:00" are all binary
//! searches. All times are Unix seconds (UTC); presentation in local time is
//! the host's job.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Programme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    /// 1-based season number when known.
    pub season: Option<u32>,
    /// 1-based episode number when known.
    pub episode: Option<u32>,
}

impl Programme {
    pub fn duration(&self) -> i64 {
        (self.stop - self.start).max(0)
    }
    pub fn is_on_air(&self, at: i64) -> bool {
        self.start <= at && at < self.stop
    }
    /// 0.0 … 1.0 progress through the programme at time `at`.
    pub fn progress(&self, at: i64) -> f32 {
        let d = self.duration();
        if d <= 0 {
            return 0.0;
        }
        ((at - self.start) as f32 / d as f32).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EpgChannel {
    pub id: String,
    pub display_names: Vec<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpgIndex {
    channels: HashMap<String, EpgChannel>,
    /// channel id → programmes sorted by start.
    programmes: HashMap<String, Vec<Programme>>,
    /// lower-cased display name → channel id, for playlists without `tvg-id`.
    by_name: HashMap<String, String>,
}

impl EpgIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_channel(&mut self, channel: EpgChannel) {
        for name in &channel.display_names {
            self.by_name
                .entry(normalise_name(name))
                .or_insert_with(|| channel.id.clone());
        }
        self.channels.insert(channel.id.clone(), channel);
    }

    /// Insert one programme. Call [`EpgIndex::finish`] after bulk insertion.
    pub fn add_programme(&mut self, channel_id: &str, programme: Programme) {
        if programme.stop <= programme.start {
            return;
        }
        self.programmes
            .entry(channel_id.to_string())
            .or_default()
            .push(programme);
    }

    /// Sort programmes and drop exact duplicates. Idempotent.
    pub fn finish(&mut self) {
        for list in self.programmes.values_mut() {
            list.sort_by_key(|p| (p.start, p.stop));
            list.dedup_by(|a, b| a.start == b.start && a.title == b.title);
        }
    }

    /// Merge another index into this one (for playlists with several EPG URLs).
    pub fn merge(&mut self, other: EpgIndex) {
        for (_, ch) in other.channels {
            self.add_channel(ch);
        }
        for (id, list) in other.programmes {
            self.programmes.entry(id).or_default().extend(list);
        }
        self.finish();
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
    pub fn programme_count(&self) -> usize {
        self.programmes.values().map(Vec::len).sum()
    }
    pub fn channel(&self, id: &str) -> Option<&EpgChannel> {
        self.channels.get(id)
    }
    pub fn channel_ids(&self) -> impl Iterator<Item = &str> {
        self.channels.keys().map(String::as_str)
    }
    pub fn has_programmes(&self, channel_id: &str) -> bool {
        self.programmes
            .get(channel_id)
            .map(|l| !l.is_empty())
            .unwrap_or(false)
    }

    /// Find the EPG channel id for a playlist entry: exact `tvg-id` first,
    /// then a case-insensitive display-name match on the channel name.
    pub fn resolve(&self, tvg_id: Option<&str>, name: &str) -> Option<String> {
        if let Some(id) = tvg_id.map(str::trim).filter(|s| !s.is_empty()) {
            if self.programmes.contains_key(id) || self.channels.contains_key(id) {
                return Some(id.to_string());
            }
        }
        self.by_name.get(&normalise_name(name)).cloned()
    }

    pub fn programmes(&self, channel_id: &str) -> &[Programme] {
        self.programmes
            .get(channel_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The programme on air at `at`, and the one after it.
    pub fn now_next(&self, channel_id: &str, at: i64) -> (Option<&Programme>, Option<&Programme>) {
        let list = self.programmes(channel_id);
        if list.is_empty() {
            return (None, None);
        }
        // First programme whose start is > at.
        let idx = list.partition_point(|p| p.start <= at);
        let now = idx
            .checked_sub(1)
            .map(|i| &list[i])
            .filter(|p| p.is_on_air(at));
        let next = list.get(idx);
        (now, next)
    }

    /// Programmes overlapping the half-open interval `[from, to)`.
    pub fn between(&self, channel_id: &str, from: i64, to: i64) -> &[Programme] {
        let list = self.programmes(channel_id);
        if list.is_empty() || to <= from {
            return &[];
        }
        // Programmes are sorted by start; overlapping ones start before `to`.
        let end = list.partition_point(|p| p.start < to);
        // Walk back to include programmes that started before `from` but end after it.
        let mut begin = list[..end].partition_point(|p| p.start < from);
        while begin > 0 && list[begin - 1].stop > from {
            begin -= 1;
        }
        &list[begin..end]
    }

    /// Case-insensitive title search across all channels, returning
    /// `(channel_id, programme)` pairs for programmes ending after `after`.
    pub fn search(&self, query: &str, after: i64, limit: usize) -> Vec<(&str, &Programme)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(&str, &Programme)> = self
            .programmes
            .iter()
            .flat_map(|(id, list)| list.iter().map(move |p| (id.as_str(), p)))
            .filter(|(_, p)| p.stop > after && p.title.to_lowercase().contains(&q))
            .collect();
        hits.sort_by_key(|(_, p)| p.start);
        hits.truncate(limit);
        hits
    }

    /// Drop programmes that ended before `before` to bound memory on long-running sessions.
    pub fn prune_before(&mut self, before: i64) {
        for list in self.programmes.values_mut() {
            list.retain(|p| p.stop > before);
        }
        self.programmes.retain(|_, l| !l.is_empty());
    }
}

fn normalise_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(start: i64, stop: i64, title: &str) -> Programme {
        Programme {
            start,
            stop,
            title: title.into(),
            subtitle: None,
            description: None,
            category: None,
            icon: None,
            season: None,
            episode: None,
        }
    }

    fn index() -> EpgIndex {
        let mut e = EpgIndex::new();
        e.add_channel(EpgChannel {
            id: "c1".into(),
            display_names: vec!["BBC One".into(), "BBC 1".into()],
            icon: None,
        });
        e.add_programme("c1", prog(200, 300, "C"));
        e.add_programme("c1", prog(0, 100, "A"));
        e.add_programme("c1", prog(100, 200, "B"));
        e.add_programme("c1", prog(100, 200, "B")); // duplicate
        e.add_programme("c1", prog(500, 400, "Invalid")); // stop before start
        e.finish();
        e
    }

    #[test]
    fn now_next_and_gaps() {
        let e = index();
        assert_eq!(e.programme_count(), 3);
        let (now, next) = e.now_next("c1", 150);
        assert_eq!(now.unwrap().title, "B");
        assert_eq!(next.unwrap().title, "C");
        let (now, next) = e.now_next("c1", 0);
        assert_eq!(now.unwrap().title, "A");
        assert_eq!(next.unwrap().title, "B");
        let (now, next) = e.now_next("c1", 300);
        assert!(now.is_none());
        assert!(next.is_none());
        let (now, next) = e.now_next("c1", -5);
        assert!(now.is_none());
        assert_eq!(next.unwrap().title, "A");
        assert_eq!(e.now_next("nope", 0), (None, None));
    }

    #[test]
    fn between_includes_overlaps() {
        let e = index();
        let titles: Vec<_> = e
            .between("c1", 150, 250)
            .iter()
            .map(|p| p.title.as_str())
            .collect();
        assert_eq!(titles, vec!["B", "C"]);
        let titles: Vec<_> = e
            .between("c1", 0, 1000)
            .iter()
            .map(|p| p.title.as_str())
            .collect();
        assert_eq!(titles, vec!["A", "B", "C"]);
        assert!(e.between("c1", 300, 400).is_empty());
        assert!(e.between("c1", 50, 50).is_empty());
    }

    #[test]
    fn resolves_by_id_then_name() {
        let e = index();
        assert_eq!(e.resolve(Some("c1"), "whatever").as_deref(), Some("c1"));
        assert_eq!(e.resolve(None, "  bbc   one ").as_deref(), Some("c1"));
        assert_eq!(e.resolve(Some("missing"), "BBC 1").as_deref(), Some("c1"));
        assert_eq!(e.resolve(None, "ITV"), None);
    }

    #[test]
    fn search_and_prune() {
        let mut e = index();
        let hits = e.search("b", 0, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.title, "B");
        e.prune_before(150);
        assert_eq!(e.programme_count(), 2);
    }

    #[test]
    fn progress() {
        let p = prog(100, 200, "x");
        assert_eq!(p.progress(150), 0.5);
        assert_eq!(p.progress(0), 0.0);
        assert_eq!(p.progress(999), 1.0);
    }
}
