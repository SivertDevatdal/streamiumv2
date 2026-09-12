//! # streamium-core
//!
//! The platform-independent heart of Streamium. Everything in this crate is
//! **sans-IO**: it never opens a socket, touches the file system, or spawns a
//! thread. The host platform (Swift on Apple, Kotlin on Android, Rust/C++ on
//! desktop) performs networking with its native HTTP stack and hands bytes to
//! this crate, which parses, indexes and answers questions.
//!
//! Why sans-IO?
//! * Every platform gets its native network behaviour for free (proxies, VPN
//!   profiles, App Transport Security, background transfers, captive portals).
//! * The core is deterministic and trivially unit-testable on any machine.
//! * No TLS stack, no async runtime, no thread pool to cross-compile. The
//!   resulting static library is small and links into anything.
//!
//! Modules:
//! * [`m3u`]    — tolerant parser for `#EXTM3U` playlists with `tvg-*` attributes.
//! * [`xtream`] — URL builders and forgiving response models for the Xtream
//!   Codes `player_api.php` protocol.
//! * [`xmltv`]  — streaming XMLTV parser (plain or gzip) into an [`epg::EpgIndex`].
//! * [`epg`]    — compact in-memory EPG index with now/next and range queries.
//! * [`catalog`]— channel catalog with groups, search and favourites.

pub mod catalog;
pub mod epg;
pub mod error;
pub mod m3u;
pub mod model;
pub mod xmltv;
pub mod xtream;

pub use error::CoreError;
