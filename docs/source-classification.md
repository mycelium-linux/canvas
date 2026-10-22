# Canvas Data Source Classification

Each candidate data source belongs to exactly one ingestion path. The paths
differ by orders of magnitude in implementation cost, so an undifferentiated
list reads as one project when it is really several plus a pile of afternoons.

## Paths

### Path 1: Binding — StreamEnvelope → Lua binder → widget snapshot → InputPort
- **Cost**: Near-free. A `.lua` file in `binders/`, no rebuild.
- **Constraint**: The binder receives an already-decoded envelope (JSON in the
  `payload` field). It is not a decode path and must not become one. Lua cannot
  perform I/O.
- **Use for**: Small, structured, frequently updating numeric or text data.
- **Pattern**: External relay (Python feed) → `stream.sock` → Lua adapter → widget.

### Path 2: Asset — AssetRef{sha256, media_type, byte_length} → SurfaceContent::Asset/NodeContent::Image/Mesh
- **Cost**: Medium. Content-addressed, fetched once, rendered natively.
- **Use for**: Images, PDFs, renderings, static meshes.
- **Pattern**: Local resolver process → content-addressed store → AssetRef → Canvas.

### Path 3: Native Surface — continuous decode or GPU residency
- **Cost**: High. Each path-3 source is its own feature with frame pacing,
  backpressure, and GPU lifetime concerns.
- **Use for**: Video/NVR, sound, FEA/CFD, geospatial, terminal streams.

## Classification

### Path 1 (binding) — already partially implemented
| Source | Status | Notes |
|--------|--------|-------|
| RSS feeds | ✅ Ready | Feed script (`canvas-sse-feed.py`) + `lockout-v1`/`widget-snapshot-v1` adapters |
| Prometheus metrics | ✅ Done (dock-cov.3.1.1) | `canvas-prometheus-feed.py` + `canvas.prometheus.v1` contract |
| Financial/OHLCV (market data) | ✅ Exists | `market-ohlcv-v1.lua` binder, `canvas.widget.snapshot.v1` contract |
| SSE from Alfalfa/Elmers/MCP-Bridge | ✅ Ready | `canvas-sse-feed.py` relay |
| Bounded text / status | ✅ Exists | `lockout-v1.lua` renders structured status tables |
| Alfalfa repo status | ✅ Live | `canvas-repo-feed.py` → `alfalfa/repo-status` stream |
| Beads activity | ✅ Live | `canvas-beads-feed.py` → `alfalfa/beads-status` stream |
| Briefing beats | ✅ Exists | `canvas.briefing.beat.v1` contract, `apply_briefing_binder` |

### Path 2 (asset) — implemented
| Source | Status | Notes |
|--------|--------|-------|
| Images (PNG, JPEG, SVG) | ✅ Real | `media.rs` 533 lines: MediaLimits, MediaCacheKey, MediaPlan |
| PDFs | ✅ Real | PDF rasterization in `media.rs` |
| Rendered charts | ✅ Real | ChartProgram renders to canvas framebuffer |
| Static 3D meshes | ✅ Real | `NodeContent::Mesh { asset: AssetRef }` |

### Path 3 (native surface) — partially declared
| Source | Status | Cost | Notes |
|--------|--------|------|-------|
| Video / NVR (RTSP/HLS) | ⚠️ Declared only | Quarter | `SurfaceContent::Stream { url }` exists in Rust and MCP schema, but has one construction site (`main.rs:7806`) and no decoder. No ffmpeg/rtsp/hls in the tree. Largest genuine gap. |
| Audio / Sound | ❌ Not found | Quarter | Nothing exists for audio. |
| FEA/CFD simulation output | ❌ Undisciplined | Quarter+ | No native surface variant. Would need GPU residency + backpressure. |
| Panopticon geospatial | ⚠️ Unclear | Day–Quarter | `canvas.quant.candle.v1` contract exists as a literal. If Panopticon can emit bounded map data (not raster tiles), it's Path 1 (binding) not Path 3. Worth settling before sizing. |
| Terminal sessions | ✅ Real | Day | `SurfaceContent::Terminal { resource_id }` implemented via `terminal.rs` (PTY path). Not through the decode pipeline. |

### Remote sources

Remote feeds do not connect directly. Ingress is an owner-local Unix socket
(`stream.sock`) by design. Any remote source needs a local relay that
authenticates outward and pushes inward, keeping credentials in the relay and
out of Canvas. One relay pattern serves RSS, SSE, and financial feeds alike;
building it once is the unlock for most of Path 1.

### SurfaceContent::Unsupported

`SurfaceContent::Unsupported` exists in Rust but is absent from the published
MCP JSON schema, which lists only `project_resource, asset, chart, widget, scene,
terminal, stream`. Agents cannot express or expect it. This is minor schema
drift — note for dock-cov.3.2.

`main.rs:3822` renders `SurfaceContent::Stream { url }` as "Connecting to stream..."
plus the URL. No decoder exists, so it never connects. The
`_ => continue` in `ensure_scene_decoded` means unsupported sources degrade
legibly (a named tile) rather than silently. New Path-3 work should keep
landing in this pattern until real.
