# HDHomeRun television in Canvas

`canvas-tv` consumes an existing Unibus HDHomeRun LAN proxy. It does not
discover tuners, forward broadcasts, own the proxy, or replace Mycelium.

The initial viewer is **video-only**. Audio routing and channel-selection
controls are separate follow-up work; it does not change the 18i20 routing.

## Home-Pi: KOMO 4.1

Run as the Canvas owner (`ajm`), with `XDG_RUNTIME_DIR=/run/user/1000`:

```sh
canvas-tv komo4 http://192.168.2.74:5004/auto/v4.1
```

In a separate shell, use the existing Canvas control socket:

```sh
printf 'surface add-live-video komo4 komo4 960 540\nsurface center komo4 0.60\n' | canvasctl
```

The installed TV view is the `television` workspace. The running decoder is
a transient user unit, `canvas-tv-komo4.service`; it does not start on reboot.

```sh
systemctl --user status canvas-tv-komo4
journalctl --user -u canvas-tv-komo4 -n 30
systemctl --user stop canvas-tv-komo4
```

Stopping the unit releases its tuner. Closing the surface alone currently
does **not** stop decoding. The CONNECT DUO has two tuners; the proxy does
not yet share an existing channel stream between viewers. Do not interrupt
another viewer to acquire a tuner.

## Decode and rendering boundary

KOMO is MPEG-2/AC-3. Home-Pi's installed GStreamer stack exposes H.264/H.265
hardware decoders, but no MPEG-2 hardware decoder. This viewer therefore uses
FFmpeg software video decoding, conditional deinterlacing, aspect-preserving
scaling and padding to 960×540 at 24 fps. Canvas uploads/renders those frames
through its existing GPU-backed native-video surface.

The shared `canvas-media-edge` publisher owns the size sidecar and atomic
RGBA frame replacement in the user runtime directory. Decoder geometry,
published dimensions and exact byte count are coupled. There is one reusable
frame allocation and no application-level frame queue or SD-card frame writes.
No partial frame is published. Decode errors are visible in the unit journal;
this initial viewer exits rather than silently retrying or acquiring another tuner.

Only explicit private/loopback IPv4 HTTP channel endpoints are accepted.
No URLs or executable commands arrive through a widget or Lua payload.
