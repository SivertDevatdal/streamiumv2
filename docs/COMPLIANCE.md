# Compliance and legal posture

This document is engineering guidance, not legal advice. Have a lawyer with
media and App Store experience review the product before launch, especially
for the EU/UK, where liability for "facilitating" infringement is assessed
differently from the US.

## The principle

Streamium is a media player, in the same category as VLC, Infuse or Plex's
client. A player is legal; what makes an app a target is **facilitating**
access to infringing content. Every product decision below removes a way in
which the app could be said to facilitate.

## Hard rules (enforced in code and review)

1. **No bundled content.** The app ships with zero playlists, zero server
   addresses, zero channels, zero EPG data. There is no "sample", "demo" or
   "popular providers" list. `Source` values only ever come from user input.
2. **No discovery of third-party IPTV sources.** No search, directory,
   scraping, QR-code marketplace or "paste a code" feature that resolves to
   someone else's server. The one directory we integrate is the community
   internet-radio index (Radio Browser), which lists stations' own public
   streams.
3. **No server-side involvement in streams.** Streamium never proxies,
   caches, relays, re-hosts, records to a cloud, or transcodes user streams
   on infrastructure we operate. Playback is strictly device ↔ user's source.
4. **No circumvention.** Playlist lines carrying DRM licence keys
   (`#KODIPROP:inputstream.adaptive.license_key`) are ignored, not stored or
   applied. We do not implement ClearKey or Widevine key injection from
   playlists. FairPlay is only used with a licence server the user's provider
   operates.
5. **No piracy vocabulary.** App name, App Store keywords, screenshots,
   website and support articles never mention specific pirated channels,
   "free premium TV", "all channels", "sports without subscription" or
   provider brands. Screenshots use our own test streams.
6. **User-supplied credentials only.** Xtream and playlist credentials are
   stored in the Keychain on device and never leave it. No analytics event
   ever contains a source URL, host name, channel name or programme title.
7. **Abuse channel.** A published contact for rights holders and a written
   repeat-infringer policy, even though we host nothing; it shows good faith
   and is expected by App Review.

## Apple App Review

Guidelines that get IPTV players rejected, and how Streamium answers them:

| Guideline | Risk | Answer |
|---|---|---|
| 5.2.3 Audio/Video Downloading | "Facilitates illegal file sharing" | No sources shipped, no discovery, no download of third-party streams (recording is limited to sources the user marks as own/permitted and is off by default). |
| 5.2.2 Third-Party Sites/Services | Uses content without permission | The app is a client for the user's own services; explain this in the review notes. |
| 4.2 Minimum Functionality | "Just a wrapper around a playlist" | Local library, network shares, media servers, radio, podcasts, tuner support. |
| 2.1 App Completeness | Reviewer cannot test | `scripts/mock-provider.py` generates a self-hosted demo service: generated test-pattern channels, a movie and an XMLTV guide, over both Xtream and M3U. Host it and give the reviewer that address. Never a third-party IPTV account. |
| 5.1.1 Data Collection | Watching habits | We collect none. Privacy nutrition label: no data collected. |
| ATS exception | `NSAllowsArbitraryLoads` | Justify: users connect to private media servers and tuners on their own networks, which commonly use plain HTTP. |

Review notes should state plainly: "Streamium contains no content and no
links to content. It plays media from sources the user configures, such as
their home media server, their broadcaster's service or files they own."

## The legitimate feature set

These are not decoration; each is a real reason to use the app and together
they make it a general-purpose player.

- **Personal library**: play video and audio from Files, iCloud Drive,
  external drives (macOS/iPadOS), with resume positions and folder browsing.
- **Network shares**: SMB and WebDAV browsing (phase 2), the way Infuse and
  VLC do it.
- **Personal media servers**: Jellyfin and Emby (open protocol), Plex
  (official API, user's own token), including their live-TV and DVR
  features.
- **Over-the-air tuners**: HDHomeRun (documented local HTTP API, its own
  lineup and guide) and Tvheadend. Legal, licence-free live TV for anyone
  with an antenna.
- **Broadcaster and telco services**: many public broadcasters and ISPs
  publish HLS streams and XMLTV guides for their subscribers; those are
  exactly the "bring your own playlist" case done legitimately.
- **Internet radio and podcasts**: Radio Browser directory, RSS podcast
  subscriptions with episode downloads (the user is entitled to those
  downloads by the feed's terms).
- **Player quality**: hardware decoding, low-latency live buffer, catch-up
  and time-shift for sources that offer it, multi-audio and subtitle
  selection, Picture in Picture, AirPlay, keyboard/remote shortcuts,
  channel numbers, favourites, parental PIN, multiple profiles.
- **Guide**: full-week EPG grid, search, reminders, "what's on now" widgets.

## Documents to publish before launch

- Terms of Use with an acceptable-use clause: the user represents that they
  hold the rights to any source they add.
- Privacy Policy: nothing about sources or viewing leaves the device.
- Copyright / abuse contact page and repeat-infringer policy.
- In-app first-run notice, one sentence, no dark patterns: "Streamium plays
  media from sources you add. Only add sources you are allowed to use."

## What we deliberately do not build

- Any list, feed, ranking or recommendation of IPTV providers.
- Playlist "sharing" between users through our infrastructure.
- Cloud DVR or cloud sync of playlists (device-to-device sync via the
  user's own iCloud account is acceptable because the data never touches
  our servers).
- Affiliate links, reseller panels or provider sign-up flows.
