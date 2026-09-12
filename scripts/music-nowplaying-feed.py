#!/usr/bin/env python3
"""
Music now-playing feed for Canvas.

Reads current MPD playback state via `mpc` and publishes a stream event to
Canvas's Unix datagram socket (stream.sock) using the
`canvas.music.nowplaying.v1` contract.

Run:
  python3 scripts/music-nowplaying-feed.py \
    --socket ~/.local/state/dock/canvas/stream.sock \
    --project dock \
    --interval 2

The --interval controls how often to poll MPD (seconds). Default 2.
Set to 0 to send only on MPD state changes (uses `mpc idle`).
"""
import argparse
import json
import os
import socket
import subprocess
import sys
import time
import hashlib

CONTRACT = "canvas.music.nowplaying.v1"
SCHEMA_VERSION = 1
MAX_DATAGRAM = 2048


def run_mpc(args):
    """Run mpc and return parsed output. Returns empty list on failure."""
    try:
        result = subprocess.run(
            ["mpc"] + args,
            capture_output=True, text=True, timeout=5
        )
        if result.returncode != 0:
            return None
        return result.stdout.strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return None


def get_now_playing(project):
    """Get current MPD playback state. Returns a dict or None if not playing."""
    # mpc fields: Artist, Title, Album, date, track, time, elapsed, duration
    # We use --format to get structured output
    fmt = "%artist%\\n%title%\\n%album%\\n%elapsed%\\n%time%\\n%file%"
    output = run_mpc(["--format", fmt, "current"])
    if not output:
        return {
            "project": project,
            "track": "Not playing",
            "artist": "",
            "album": "",
            "elapsed_sec": 0,
            "duration_sec": 0,
            "is_playing": False,
        }

    lines = output.split("\n")
    # Pad to expected length
    while len(lines) < 6:
        lines.append("")

    artist = lines[0] or ""
    title = lines[1] or ""
    album = lines[2] or ""
    elapsed_str = lines[3] or "0"
    duration_str = lines[4] or "0:00"

    def parse_time(t):
        """Parse MM:SS or seconds into seconds float."""
        if ":" in t:
            parts = t.split(":")
            if len(parts) == 2:
                try:
                    return float(parts[0]) * 60 + float(parts[1])
                except ValueError:
                    return 0.0
        try:
            return float(t)
        except ValueError:
            return 0.0

    elapsed = parse_time(elapsed_str)
    duration = parse_time(duration_str)

    # Check playback status
    status = run_mpc(["status"])
    is_playing = False
    if status:
        for line in status.split("\n"):
            if line.startswith("[playing]"):
                is_playing = True
                break
            elif line.startswith("[paused]"):
                is_playing = False
                break

    return {
        "project": project,
        "track": title or (artist and "Untitled" or "Not playing"),
        "artist": artist,
        "album": album,
        "elapsed_sec": round(elapsed, 1),
        "duration_sec": round(duration, 1),
        "is_playing": is_playing,
    }


def get_album_art():
    """Try to extract album art from MPD. Returns sha256 hash or None."""
    # mpc can output album art with --format or the 'albumart' command
    # For now, skip - album art is published as a separate surface
    return None


def send_event(sock_path, payload):
    """Send a stream envelope to Canvas's Unix datagram socket."""
    now_ms = int(time.time() * 1000)

    # Use a monotonically increasing sequence number per producer
    # We use a hash of (path, timestamp) to approximate uniqueness
    seq = int.from_bytes(hashlib.md5(str(now_ms).encode()).digest()[:8], "big")

    envelope = {
        "schema_version": SCHEMA_VERSION,
        "owner": "music",
        "stream": "now-playing",
        "contract": CONTRACT,
        "sequence": seq,
        "emitted_at_ms": now_ms,
        "payload": payload,
    }

    data = json.dumps(envelope).encode()
    if len(data) > MAX_DATAGRAM:
        return False

    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.connect(sock_path)
        sock.send(data)
        sock.close()
        return True
    except (FileNotFoundError, ConnectionRefusedError, OSError):
        return False


def main():
    parser = argparse.ArgumentParser(description="Music now-playing feed for Canvas")
    parser.add_argument("--socket", required=True,
                        help="Path to stream.sock")
    parser.add_argument("--project", default="dock",
                        help="Project name")
    parser.add_argument("--interval", type=float, default=2.0,
                        help="Polling interval in seconds (0 = event-driven via mpc idle)")
    args = parser.parse_args()

    last_payload = None
    last_seq = 0

    if args.interval <= 0:
        # Event-driven mode: use mpc idle to wait for state changes
        print("Event-driven mode: waiting for MPD state changes...")
        # mpc idle blocks until player state changes
        # We poll every 1s to also catch position updates
        poll_interval = 1.0
        while True:
            # Check for MPD state changes
            idle = subprocess.run(
                ["mpc", "idle"],
                capture_output=True, text=True, timeout=10
            )
            payload = get_now_playing(args.project)
            if payload != last_payload:
                last_payload = payload
                if send_event(args.socket, payload):
                    print(f"Sent: {payload['artist']} - {payload['track']}", flush=True)
            time.sleep(poll_interval)
    else:
        # Polling mode
        print(f"Polling mode: interval={args.interval}s", flush=True)
        while True:
            payload = get_now_playing(args.project)
            if payload != last_payload:
                last_payload = payload
                if send_event(args.socket, payload):
                    print(f"Sent: {payload['artist']} - {payload['track']}", flush=True)
            time.sleep(args.interval)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nStopped.", flush=True)
        sys.exit(0)
