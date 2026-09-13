# Streamium for Windows — test build

Thank you for testing this. It is an early build: the parts that find your
channels and read your guide are finished and well tested, the window around
them is new, and video is played by mpv or VLC rather than by Streamium
itself. What is most useful to us is whether **your** provider, **your**
channels and **your** guide come out right, on **your** connection.

Streamium ships no content. It plays the playlists, servers and files you
add yourself, and that you are entitled to use.

## Install

1. Unzip the folder anywhere — the Desktop or Downloads is fine. Nothing is
   installed and nothing is written outside your own user folder.
2. Install a video player, unless you already have one. **mpv** is the one to
   pick: <https://mpv.io/installation/> (on Windows, "scoop" or the plain
   build both work). VLC also works but cannot receive the custom HTTP
   headers some providers insist on.
3. Double-click `streamium.exe`.

### "Windows protected your PC"

The build is not code-signed, so SmartScreen will warn about it the first
time. Click **More info**, then **Run anyway**. If you would rather not, say
so — that is a reasonable thing to refuse, and a signed build is on the list.

## First run

**Add source** opens a form with three choices:

- **Xtream account** — server address, username and password, exactly as your
  provider gave them. The address can be pasted with or without `http://`,
  and with or without a trailing `/player_api.php`.
- **M3U playlist** — an address, or a path to a `.m3u` file on this PC.
- **Folder of files** — any folder of video or music files, if you would
  rather test without an account.

Channels appear on the right, groups on the left, and the guide fills in
underneath each channel name as it downloads. A big provider takes a few
seconds for the channels and longer for the guide; the bar at the bottom says
what it is doing.

Double-click a channel to play it, or select it and use the buttons on the
right.

## What is worth testing

- **Does your provider load at all?** How long does it take, and does the
  count at the bottom look right?
- **Are the channel names, numbers and groups correct?** Anything mangled,
  out of order, or missing?
- **Is the guide right?** Does "now" match what is actually on? Is it in your
  local time? Do channels that should have a guide have one?
- **Does search behave?** Try a channel name, part of a name, and a number.
- **Analyse stream.** Select a channel and press it. Streamium pulls the first
  few seconds itself and reports what its demuxer found: the programs, the
  video and audio codecs, how long the first key frame took, and any errors.
  Press **Copy report** and send that back — it is the single most useful
  thing you can give us, and the account details are removed from it.
- **Does Play work,** and how long from click to picture?
- **Favourites** (the star), and whether they are still there after a restart.

Both good and bad results are worth reporting. "All 1 400 channels loaded,
guide correct, first key frame 380 ms" is as useful as a failure.

## When something goes wrong

Open **Log** in the top-right corner and press **Copy log**. That, plus what
you were doing, is usually enough. The log holds addresses and error
messages; it does not hold your password.

Known limits in this build, no need to report them:

- Video plays in mpv/VLC, not inside the Streamium window.
- No catch-up, no series browsing, no subtitle selection, no recording.
- Channel logos are not shown yet.
- Movies are only loaded if you tick the box when adding the account.

## Your details

Settings live in `%APPDATA%\Streamium\config.json` — paste that into the
address bar of Explorer to find it. **Your provider password is stored there
in plain text**, as it is in most players of this kind; anyone who can read
your user profile can read it. Encrypting it with Windows' own data
protection is on the list.

To remove Streamium completely: delete the folder you unzipped, and delete
`%APPDATA%\Streamium`.

## Building it yourself

Install [Rust](https://rustup.rs/), then:

```powershell
git clone https://github.com/sivertdevatdal/streamiumv2
cd streamiumv2
cargo run -p streamium-desktop --release
```

Nothing else is needed on Windows — no C toolchain, no vcpkg, no system
libraries. On Linux the same command works once the windowing headers are
installed (`libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`,
`libxcursor-dev`, `libxrandr-dev`, `libxi-dev`, `libgl1-mesa-dev`).

Every push builds this on a Windows runner; the finished `streamium.exe` is
attached to the run as an artifact named `streamium-windows-x86_64`.
