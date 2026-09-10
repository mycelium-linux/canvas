#!/usr/bin/env python3
"""Pace a Canvas briefing: emit one beat event per interval over the stream socket.

Each beat is a UDP datagram to Canvas's stream.sock with contract
`canvas.briefing.beat.v1`. The briefing binder (config/briefings/<name>.lua)
receives the event and returns a BriefingBeat with workspace patches + TTS
narration text.

The narration is spoken via `jarvis text-turn` (TTS through Bifrost) while
the workspace patches update the Canvas display. Audio and visual are
decoupled: the narration provides context while the screen shows state —
they complement, not duplicate.

NOTE: text-turn speaks immediately without caching. Long-term, TTS output
should be cached and played back (see bead: briefing-tts-cache).

Usage:
  canvas-briefing-run --briefing intro-v1 --every 5.0
  canvas-briefing-run --beat 3             # send a single beat
  canvas-briefing-run --no-tts             # skip narration
"""
import argparse, json, os, socket, sys, time, subprocess

def find_jarvis():
    """Locate the jarvis sound daemon binary."""
    for path in [
        os.path.join(os.environ.get("HOME", ""), ".local/bin/jarvis"),
        os.path.join(os.path.dirname(__file__), "..", "..", "JARVIS", "target", "debug", "jarvis"),
        os.path.join(os.environ.get("HOME", ""), "src/fpl/JARVIS/target/debug/jarvis"),
    ]:
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return path
    return None

def load_narration(briefing_name):
    """Extract beat-by-beat narration text from the Lua briefing script.

    The producer reads the Lua source and parses out the `narrate` strings so
    it can speak them without waiting for Canvas to round-trip the beat. This
    keeps audio ahead of the visual change, which is the cue that the beat
    is advancing rather than the words on the screen being read aloud.
    """
    paths = [
        os.path.join(os.environ.get("HOME", ""), "Library/Application Support/Dock", "config/briefings", f"{briefing_name}.lua"),
        os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "config", "briefings", f"{briefing_name}.lua"),
    ]
    source = None
    for path in paths:
        if os.path.isfile(path):
            with open(path, "r") as f:
                source = f.read()
            break
    if source is None:
        return {}

    narrate = {}
    beat = 0
    for line in source.splitlines():
        line = line.strip()
        # Match: [N] = {  (start of a beat table)
        import re
        m = re.match(r'^\[(\d+)\]\s*=\s*\{', line)
        if m:
            beat = int(m.group(1))
        # Match: narrate = "some text",
        m2 = re.match(r'^narrate\s*=\s*"(.+)",?\s*$', line)
        if m2 and beat is not None:
            narrate[beat] = m2.group(1)
    return narrate

def speak_text(text, jarvis_path=None):
    """Speak narration text for decoupling audio from visual.

    Uses macOS `say -v Daniel` directly for immediate playback. This is the
    fallback path — the full JARVIS sound daemon (text-turn with Bifrost TTS
    cache + PCM playback) is preferred when JARVIS_BIFROST_API_KEY is set,
    but requires the daemon running.

    Audio and visual are decoupled: narration cues transitions while the screen
    shows state. They complement rather than duplicate.
    """
    if not text:
        return

    # Try full JARVIS path first (higher quality, proper audio pipeline)
    if jarvis_path:
        try:
            env = {
                **os.environ,
                "JARVIS_VOICE_TRANSCRIPT_LOG_DIR": os.path.join(
                    os.environ.get("HOME", ""), "Library/Application Support/JARVIS/transcripts"
                ),
            }
            result = subprocess.run(
                [jarvis_path, "text-turn", text],
                capture_output=True, text=True, timeout=15, env=env,
            )
            if result.returncode == 0:
                return
        except Exception:
            pass

    # Fallback: macOS say with Daniel (British, natural cadence)
    try:
        subprocess.run(["say", "-v", "Daniel", text], timeout=15)
    except Exception as e:
        print(f"canvas-briefing-run: TTS failed: {e}", file=sys.stderr)

def main():
    home = os.environ.get("HOME", "")
    default_socket = os.path.join(home, "Library/Application Support/Dock/canvas/stream.sock")
    ap = argparse.ArgumentParser(description="Pace a Canvas briefing over the stream socket.")
    ap.add_argument("--socket", default=default_socket)
    ap.add_argument("--owner", default="machine")
    ap.add_argument("--stream", default="briefing")
    ap.add_argument("--briefing", default="intro-v1")
    ap.add_argument("--beat", type=int, default=None, help="Send only this one beat")
    ap.add_argument("--every", type=float, default=5.0, help="Seconds between beats")
    ap.add_argument("--max-beats", type=int, default=128)
    ap.add_argument("--no-tts", action="store_true", help="Skip TTS narration")
    args = ap.parse_args()

    jarvis_path = None if args.no_tts else find_jarvis()
    if jarvis_path:
        print(f"canvas-briefing-run: TTS via {jarvis_path}", file=sys.stderr)
    elif not args.no_tts:
        print("canvas-briefing-run: no jarvis binary found, TTS disabled", file=sys.stderr)

    if not os.path.exists(args.socket):
        sys.exit(f"no Canvas stream socket at {args.socket} - start Canvas first")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    schema_version = 1
    contract = "canvas.briefing.beat.v1"
    sequence = 0

    # Read narration texts from the briefing Lua script so the producer
    # can speak them before the visual beat lands.
    narrate_by_beat = load_narration(args.briefing)

    if args.beat is not None:
        sequence += 1
        beat_idx = args.beat
        event = {
            "schema_version": schema_version, "owner": args.owner,
            "stream": args.stream, "contract": contract,
            "sequence": sequence, "emitted_at_ms": int(time.time() * 1000),
            "payload": {"beat": beat_idx, "briefing": args.briefing}
        }
        body = json.dumps(event, separators=(",", ":")).encode()
        sock.sendto(body, args.socket)
        print(f"sent beat {beat_idx}", file=sys.stderr)
        if beat_idx in narrate_by_beat:
            speak_text(narrate_by_beat[beat_idx], jarvis_path)
        return

    print(f"canvas-briefing-run: {args.briefing} -> {args.socket}", file=sys.stderr)
    beat = 0
    while beat < args.max_beats:
        sequence += 1
        event = {
            "schema_version": schema_version, "owner": args.owner,
            "stream": args.stream, "contract": contract,
            "sequence": sequence, "emitted_at_ms": int(time.time() * 1000),
            "payload": {"beat": beat, "briefing": args.briefing}
        }
        body = json.dumps(event, separators=(",", ":")).encode()
        if len(body) > 64 * 1024:
            print(f"beat {beat} too large, skipping", file=sys.stderr)
        else:
            try:
                sock.sendto(body, args.socket)
                print(f"beat {beat}", file=sys.stderr)
            except OSError as e:
                print(f"send failed: {e}", file=sys.stderr)
        # Speak the narration — audio and visual are decoupled channels.
        if beat in narrate_by_beat:
            speak_text(narrate_by_beat[beat], jarvis_path)
        beat += 1
        time.sleep(args.every)

if __name__ == "__main__":
    main()
