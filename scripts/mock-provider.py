#!/usr/bin/env python3
"""A local stand-in for an IPTV provider, for testing Streamium end to end.

It speaks enough of the Xtream Codes protocol to exercise the whole app, and
serves real video so playback can actually be seen:

    ./scripts/mock-provider.py                 # serves on http://localhost:8080

Then in Streamium, add a source:

    Xtream account   server http://localhost:8080, any username and password
    Playlist (M3U)   http://localhost:8080/get.php?username=demo&password=demo

Channels are generated test patterns, distinguishable by eye and by ear, so a
wrong stream is obvious. It also provides an XMLTV guide, a VOD movie, and
both transport stream and HLS variants of every channel, which covers both
playback engines.

This exists because Streamium ships no sources: it is the demo source to hand
to App Review, and the way to verify the app without a real provider.

Requires ffmpeg on PATH. Media is generated once into a temporary directory.
"""

import base64
import http.server
import json
import os
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from datetime import datetime, timedelta, timezone

PORT = int(os.environ.get("PORT", "8080"))
DURATION = int(os.environ.get("DURATION", "20"))   # seconds of source media per channel
SIZE = os.environ.get("SIZE", "640x360")
FPS = 25

# Each channel gets a different pattern and audio pitch so the wrong stream is
# immediately obvious.
CHANNELS = [
    {"id": 101, "name": "Demo One HD",   "group": "Demo Channels", "epg": "demo.one",   "src": "testsrc2",   "freq": 440},
    {"id": 102, "name": "Demo Two HD",   "group": "Demo Channels", "epg": "demo.two",   "src": "smptebars",  "freq": 660},
    {"id": 103, "name": "Demo Three HD", "group": "Demo Extra",    "epg": "demo.three", "src": "rgbtestsrc", "freq": 880},
]
MOVIES = [{"id": 501, "name": "Demo Movie", "group": "Demo Movies", "ext": "mp4"}]

MEDIA = tempfile.mkdtemp(prefix="streamium-mock-")


def run(cmd):
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write(" ".join(cmd) + "\n" + result.stderr[-2000:] + "\n")
        raise SystemExit("ffmpeg failed")


def build_media():
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg is required: brew install ffmpeg")
    print(f"Generating {DURATION}s of test media per channel in {MEDIA} ...")
    for channel in CHANNELS:
        ts = os.path.join(MEDIA, f"{channel['id']}.ts")
        run([
            "ffmpeg", "-v", "error", "-y",
            "-f", "lavfi", "-i", f"{channel['src']}=size={SIZE}:rate={FPS}:duration={DURATION}",
            "-f", "lavfi", "-i", f"sine=frequency={channel['freq']}:sample_rate=48000:duration={DURATION}",
            "-c:v", "libx264", "-preset", "ultrafast", "-g", str(FPS), "-pix_fmt", "yuv420p", "-b:v", "800k",
            "-c:a", "aac", "-b:a", "96k", "-ar", "48000", "-ac", "2",
            "-muxrate", "1800k", "-f", "mpegts", ts,
        ])
        # An HLS rendition of the same content, to exercise the AVPlayer path.
        hls_dir = os.path.join(MEDIA, f"hls{channel['id']}")
        os.makedirs(hls_dir, exist_ok=True)
        run([
            "ffmpeg", "-v", "error", "-y", "-i", ts, "-c", "copy",
            "-f", "hls", "-hls_time", "2", "-hls_list_size", "0",
            "-hls_segment_filename", os.path.join(hls_dir, "seg%03d.ts"),
            os.path.join(hls_dir, "index.m3u8"),
        ])
    for movie in MOVIES:
        run([
            "ffmpeg", "-v", "error", "-y",
            "-f", "lavfi", "-i", f"testsrc2=size={SIZE}:rate={FPS}:duration={DURATION}",
            "-f", "lavfi", "-i", f"sine=frequency=330:sample_rate=48000:duration={DURATION}",
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-b:v", "800k",
            "-c:a", "aac", "-b:a", "96k", "-movflags", "+faststart",
            os.path.join(MEDIA, f"{movie['id']}.{movie['ext']}"),
        ])
    print("Media ready.")


def now():
    return datetime.now(timezone.utc)


def xmltv():
    """A guide covering the next 12 hours, so now/next always has something."""
    out = ['<?xml version="1.0" encoding="UTF-8"?>', '<tv generator-info-name="streamium-mock">']
    for channel in CHANNELS:
        out.append(f'  <channel id="{channel["epg"]}">')
        out.append(f'    <display-name>{channel["name"]}</display-name>')
        out.append("  </channel>")
    start = now().replace(minute=0, second=0, microsecond=0) - timedelta(hours=1)
    for channel in CHANNELS:
        for slot in range(13):
            begin = start + timedelta(hours=slot)
            end = begin + timedelta(hours=1)
            fmt = "%Y%m%d%H%M%S +0000"
            out.append(f'  <programme start="{begin.strftime(fmt)}" stop="{end.strftime(fmt)}" channel="{channel["epg"]}">')
            out.append(f'    <title>{channel["name"]} programme {slot + 1}</title>')
            out.append("    <desc>Generated listing from the Streamium mock provider.</desc>")
            out.append(f'    <episode-num system="onscreen">S01E{slot + 1:02d}</episode-num>')
            out.append("  </programme>")
    out.append("</tv>")
    return "\n".join(out).encode()


def base_url(handler):
    host = handler.headers.get("Host") or f"localhost:{PORT}"
    return f"http://{host}"


def playlist(handler, user, password):
    lines = [f'#EXTM3U url-tvg="{base_url(handler)}/xmltv.php?username={user}&password={password}"']
    for channel in CHANNELS:
        lines.append(
            f'#EXTINF:-1 tvg-id="{channel["epg"]}" tvg-name="{channel["name"]}" '
            f'tvg-chno="{channel["id"] - 100}" group-title="{channel["group"]}",{channel["name"]}'
        )
        lines.append(f'{base_url(handler)}/live/{user}/{password}/{channel["id"]}.ts')
    for movie in MOVIES:
        lines.append(f'#EXTINF:-1 group-title="{movie["group"]}",{movie["name"]}')
        lines.append(f'{base_url(handler)}/movie/{user}/{password}/{movie["id"]}.{movie["ext"]}')
    return ("\n".join(lines) + "\n").encode()


def api(handler, query):
    action = query.get("action", [""])[0]
    expiry = int((now() + timedelta(days=30)).timestamp())

    if not action:
        return {
            "user_info": {
                "username": query.get("username", ["demo"])[0],
                "password": query.get("password", ["demo"])[0],
                "status": "Active",
                "auth": 1,
                "exp_date": str(expiry),
                "is_trial": "0",
                "active_cons": "0",
                "max_connections": "2",
                # Deliberately mixed types and a string list: real panels vary
                # wildly here and the client must cope.
                "allowed_output_formats": ["m3u8", "ts"],
            },
            "server_info": {
                "url": handler.headers.get("Host", "localhost").split(":")[0],
                "port": str(PORT),
                "https_port": "",
                "server_protocol": "http",
                "timezone": "UTC",
                "timestamp_now": int(time.time()),
            },
        }

    if action == "get_live_categories":
        seen, out = [], []
        for channel in CHANNELS:
            if channel["group"] not in seen:
                seen.append(channel["group"])
                out.append({"category_id": str(len(seen)), "category_name": channel["group"], "parent_id": 0})
        return out

    if action == "get_live_streams":
        groups = []
        for channel in CHANNELS:
            if channel["group"] not in groups:
                groups.append(channel["group"])
        return [
            {
                "num": channel["id"] - 100,
                "name": channel["name"],
                "stream_type": "live",
                # A number here, not a string: panels are inconsistent.
                "stream_id": channel["id"],
                "stream_icon": "",
                "epg_channel_id": channel["epg"],
                "added": "1600000000",
                "category_id": str(groups.index(channel["group"]) + 1),
                "tv_archive": 1,
                "tv_archive_duration": "7",
                "direct_source": "",
                "is_adult": "0",
            }
            for channel in CHANNELS
        ]

    if action == "get_vod_categories":
        return [{"category_id": "10", "category_name": MOVIES[0]["group"], "parent_id": 0}]

    if action == "get_vod_streams":
        return [
            {
                "num": 1,
                "name": movie["name"],
                "stream_type": "movie",
                "stream_id": str(movie["id"]),
                "stream_icon": "",
                "category_id": "10",
                "container_extension": movie["ext"],
                "added": "1600000000",
                "rating": "7.5",
            }
            for movie in MOVIES
        ]

    if action in ("get_series_categories", "get_series"):
        return []

    if action in ("get_short_epg", "get_simple_data_table"):
        listings = []
        start = now().replace(minute=0, second=0, microsecond=0)
        for slot in range(4):
            begin = start + timedelta(hours=slot)
            listings.append({
                "id": str(slot),
                "title": base64.b64encode(f"Demo programme {slot + 1}".encode()).decode(),
                "description": base64.b64encode(b"Generated listing.").decode(),
                "start_timestamp": str(int(begin.timestamp())),
                "stop_timestamp": str(int((begin + timedelta(hours=1)).timestamp())),
                "has_archive": 1,
            })
        return {"epg_listings": listings}

    return []


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write(f"  {self.address_string()} {fmt % args}\n")

    def send_bytes(self, body, content_type, status=200):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        query = urllib.parse.parse_qs(parsed.query)
        user = query.get("username", ["demo"])[0]
        password = query.get("password", ["demo"])[0]

        if path == "/player_api.php" or path == "/panel_api.php":
            return self.send_bytes(json.dumps(api(self, query)).encode(), "application/json")
        if path == "/xmltv.php":
            return self.send_bytes(xmltv(), "application/xml")
        if path == "/get.php":
            return self.send_bytes(playlist(self, user, password), "application/vnd.apple.mpegurl")

        parts = [p for p in path.split("/") if p]

        # /live/<user>/<pass>/<id>.ts  or  .m3u8
        if len(parts) == 4 and parts[0] == "live":
            name = parts[3]
            stream_id, _, ext = name.rpartition(".")
            if ext == "ts":
                return self.stream_transport(stream_id)
            if ext == "m3u8":
                return self.serve_file(os.path.join(MEDIA, f"hls{stream_id}", "index.m3u8"),
                                       "application/vnd.apple.mpegurl")
        # /hls/<id>/<segment>
        if len(parts) == 3 and parts[0] == "hls":
            return self.serve_file(os.path.join(MEDIA, f"hls{parts[1]}", parts[2]), "video/mp2t")
        # /movie/<user>/<pass>/<id>.<ext>
        if len(parts) == 4 and parts[0] in ("movie", "series"):
            return self.serve_file(os.path.join(MEDIA, parts[3]), "video/mp4")

        self.send_bytes(b"not found\n", "text/plain", status=404)

    def serve_file(self, path, content_type):
        if not os.path.isfile(path):
            return self.send_bytes(b"not found\n", "text/plain", status=404)
        # HLS playlists reference segments relatively; rewrite to an absolute
        # path this server also answers.
        if path.endswith(".m3u8"):
            stream_id = os.path.basename(os.path.dirname(path)).removeprefix("hls")
            text = open(path).read()
            text = "\n".join(
                f"{base_url(self)}/hls/{stream_id}/{line}" if line and not line.startswith("#") else line
                for line in text.splitlines()
            )
            return self.send_bytes(text.encode() + b"\n", content_type)
        with open(path, "rb") as handle:
            return self.send_bytes(handle.read(), content_type)

    def stream_transport(self, stream_id):
        """Serve the channel as an endless transport stream, paced roughly at
        its own bitrate so the client sees something that behaves like live TV."""
        path = os.path.join(MEDIA, f"{stream_id}.ts")
        if not os.path.isfile(path):
            return self.send_bytes(b"no such channel\n", "text/plain", status=404)
        payload = open(path, "rb").read()
        bytes_per_second = max(len(payload) / DURATION, 16384)
        chunk = 188 * 70  # a whole number of transport packets

        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            while True:
                offset = 0
                while offset < len(payload):
                    piece = payload[offset:offset + chunk]
                    self.wfile.write(piece)
                    offset += len(piece)
                    time.sleep(len(piece) / bytes_per_second)
        except (BrokenPipeError, ConnectionResetError):
            pass


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    build_media()
    address = ("0.0.0.0", PORT)
    with Server(address, Handler) as httpd:
        print()
        print(f"Mock provider listening on http://localhost:{PORT}")
        print()
        print("  Xtream account   server:   http://localhost:%d" % PORT)
        print("                   username: demo      (any value works)")
        print("                   password: demo      (any value works)")
        print()
        print("  Playlist (M3U)   http://localhost:%d/get.php?username=demo&password=demo" % PORT)
        print()
        print(f"  {len(CHANNELS)} live channels, {len(MOVIES)} movie, and a 12 hour guide.")
        print("  Each channel is a different test pattern and audio pitch.")
        print()
        print("Ctrl-C to stop.")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nStopping.")
        finally:
            shutil.rmtree(MEDIA, ignore_errors=True)


if __name__ == "__main__":
    main()
