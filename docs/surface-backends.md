# Surface backends

Canvas still accepts several legacy `SurfaceContent`/`ContentSource` variants.
The long-term model is two runtime backends:

```text
SurfaceBackend
  Spore
    Scene
    Rgba
    GpuLease

  Native(NativeSurface)
    Terminal
    Media(LiveImage | LiveVideo | Stream)
    External
    Document
    Shell
```

The rule:

> Application state, composition and interaction belong to a Spore.
> Devices, transports, PTYs, decoders and privileged renderers belong to the
> host.

`ContentSource::backend()` is the migration boundary. It classifies every
existing variant without deleting the persisted forms yet:

| Legacy source | Backend |
| --- | --- |
| `Spore`, `Scene`, `DataViz` | `Spore` |
| `Terminal` | `Native::Terminal` |
| `LiveImage`, `LiveVideo`, `Stream` | `Native::Media` |
| `ExternalSurface` | `Native::External` |
| `Image`, `Pdf`, `Markdown` | `Native::Document` |
| `Widget`, `Prompt` | `Native::Shell` |

## Why not one Spore backend

A terminal needs a PTY. Video needs a decoder and a transport. External surfaces
carry native handles. Those belong to the host; forcing them through wasm adds
latency and authority for no benefit.

## Why not one native backend

Charts, graphs, maps, atlases, dashboards and document UIs must compose at
runtime without a compositor release. `fungos.viz` composes those inside one
Spore surface. Canvas should not learn a new widget kind per visualization.

Media should become a `MediaSource` resource, not a new surface application kind.
A native media surface can present it directly; a Spore surface can eventually
embed it through a host resource.

## Migration

1. New rendering/input code branches on `SurfaceBackend`, not every legacy kind.
2. Existing persisted content keeps decoding through the old variants.
3. New document variants need a backend category and a reason not to use it.
4. Passive documents converge on one `Native::Document` surface.
5. Media converges on `MediaSource`.
6. Spore UI absorbs charts, data views, and authored widget/module content.
