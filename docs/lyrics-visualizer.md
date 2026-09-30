# MPD Lyrics Visualizer for Canvas

A lyrics display widget with hyperspace animations and karaoke-style highlighting for Canvas.

## Features

- **Lyrics Display**: Render synchronized lyrics with timing information (LRC format)
- **Hyperspace Animation**: Multiple animation styles (Normal, Glitch, Flow, Particles, Hyperspace)
- **Karaoke Highlighting**: Word-level highlighting for karaoke-style effect
- **Smooth Scrolling**: Interpolated scroll position between updates
- **MPD Integration**: Built-in MPD client for automatic lyrics fetching
- **Widget Reusability**: Standalone widget that can be embedded in any Canvas view

## Architecture

Following FPL design tenets:

1. **Rust for Rendering**: The `lyrics_display` widget is implemented in Rust using iced canvas for high-performance rendering
2. **Lua for Data Transport**: `mpd-lyrics-v1.lua` polls MPD and produces widget mutations
3. **Separation of Concerns**: Render logic in Rust, data massaging in Lua

## Quick Start

### 1. Configure MPD

Ensure MPD is running and accessible:

```bash
# Check MPD status
systemctl status mpd

# Test connection
mpc status
```

### 2. Add Lyrics

Place `.lrc` files next to your music files:

```
/var/lib/mpd/music/
├── artist/
│   ├── album/
│   │   ├── 01 - Song.mp3
│   │   └── 01 - Song.lrc
```

**LRC Format Example**:
```lrc
[00:12.34]Hello world
[00:15.00]This is a test
[00:18.50]Another line
```

### 3. Configure Canvas

Add to your Canvas configuration (`config/projects.json`):

```json
{
  "binders": [
    {
      "id": "mpd-lyrics",
      "source": "lua",
      "script": "config/binders/mpd-lyrics-v1.lua",
      "interval_ms": 500,
      "targets": ["mpd-lyrics"]
    }
  ]
}
```

### 4. Deploy to home-pi.local

```bash
# Build for ARM64
cargo build --release --target aarch64-unknown-linux-gnu -p canvas

# Deploy
scp ./target/aarch64-unknown-linux-gnu/release/canvas ajm@home-pi.local:/opt/canvas/

# Copy config
scp -r config/* ajm@home-pi.local:/opt/canvas/config/
```

## Widget Specification

### LyricsDisplay Widget Kind

```json
{
  "schema_version": 1,
  "kind": "lyrics_display",
  "id": "mpd-lyrics",
  "title": "Track Title",
  "artist": "Artist Name",
  "lines": [
    {
      "text": "Lyric line text",
      "start_ms": 12340,
      "end_ms": 14340,
      "words": []
    }
  ],
  "position_ms": 13500,
  "duration_ms": 180000,
  "playing": true,
  "style": "hyperspace",
  "scroll_offset": 1.5,
  "highlight_intensity": 0.8,
  "chrome": "frameless",
  "data": {
    "state": "ready",
    "source": "mpd-client",
    "provenance": "live"
  }
}
```

### Animation Styles

- **`normal`**: Subtle pulse and fade effects
- **`glitch`**: Glitchy, cyberpunk aesthetic with chromatic aberration
- **`flow`**: Flowing wave animations with liquid transitions
- **`particles`**: Particles emanating from active line like stardust
- **`hyperspace`**: Deep space expansion with parallax depth (default)

### Widget Mutations

The Lua binder emits `WidgetMutation::Snapshot` mutations:

```rust
WidgetMutation::Snapshot {
    widget: WidgetKind::LyricsDisplay { ... },
    data: WidgetData {
        state: WidgetDataState::Ready,
        source: Some("mpd-client".to_string()),
        provenance: Provenance::Live,
    }
}
```

## Customization

### Modify Animation Style

Edit the widget spec or use widget mutations:

```lua
-- In your Lua binder
widget.style = "glitch"  -- or "flow", "particles", "hyperspace"
widget.highlight_intensity = 1.0  -- 0.0 to 1.0
```

### Adjust Scroll Position

```lua
-- Auto-scroll to keep active line centered
widget.scroll_offset = math.floor(active_line_index) - 3
```

### Word-Level Karaoke

For word-level timing, add words to lyric lines:

```lrc
[00:12.34]Hello [00:12.50]world
```

Parsed as:

```json
{
  "text": "Hello world",
  "start_ms": 12340,
  "end_ms": 14340,
  "words": [
    {"text": "Hello", "start_ms": 12340, "end_ms": 12500},
    {"text": "world", "start_ms": 12500, "end_ms": 14340}
  ]
}
```

## Development

### Running Locally

```bash
# Start MPD
systemctl start mpd

# Run Canvas
cargo run -p canvas --release

# Watch logs
tail -f ~/.local/state/canvas/log.txt
```

### Testing the Widget

```bash
# Test LRC parsing
cargo test -p canvas-widgets -- lyrics

# Build only the widget library
cargo build -p canvas-widgets --features live
```

### Cross-Compilation

```bash
# Build for ARM64 (home-pi.local)
cargo build --release --target aarch64-unknown-linux-gnu -p canvas

# Build for x86_64 (local testing)
cargo build --release -p canvas
```

## Troubleshooting

### MPD Connection Fails

```bash
# Check MPD is running
systemctl status mpd

# Test connection
mpc stats

# Check MPD config
cat /etc/mpd.conf
```

### Lyrics Not Loading

1. Ensure `.lrc` files are in the correct location
2. Check file permissions: `ls -la /var/lib/mpd/lyrics/`
3. Verify LRC format is correct
4. Check Canvas logs for errors

### Animation Performance

If animations are choppy:
- Reduce `scroll_offset` interpolation
- Use simpler animation styles (`normal` instead of `hyperspace`)
- Check frame rate with `cargo run --profile=release-with-debug`

## Files

- `crates/canvas-widgets/src/live.rs`: Widget rendering (Rust)
- `config/binders/mpd-lyrics-v1.lua`: MPD client (Lua)
- `config/binders/mpd-lyrics-config.json`: Widget configuration
- `config/widgets/lyrics-example.json`: Example widget spec

## Future Enhancements

- [ ] Word-level karaoke highlighting (requires text measurement)
- [ ] Online lyric fetching (MusixMatch, Genius API)
- [ ] Multi-language support
- [ ] Custom animation themes
- [ ] Touch gestures for manual scroll
- [ ] Offline mode with cached lyrics

## Related

- [FPL Design Tenets](https://github.com/anomalyco/opencode/blob/main/AGENTS.md)
- [Canvas Architecture](docs/computing-mesh-design.md)
- [Widget Schema](crates/canvas-widgets/src/lib.rs)
