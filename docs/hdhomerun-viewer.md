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
printf 'surface add-live-video komo4 komo4 1024 576\nsurface center komo4 0.60\n' | canvasctl
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
scaling and padding to 1024×576 at 24 fps. Canvas uploads/renders those frames
through its existing GPU-backed native-video surface.

The shared `canvas-media-edge` publisher owns the size sidecar and atomic
RGBA frame replacement in the user runtime directory. Decoder geometry,
published dimensions and exact byte count are coupled. There is one reusable
frame allocation and no application-level frame queue or SD-card frame writes.
No partial frame is published. Decode errors are visible in the unit journal;
this initial viewer exits rather than silently retrying or acquiring another tuner.

Only explicit private/loopback IPv4 HTTP channel endpoints are accepted.
No URLs or executable commands arrive through a widget or Lua payload.

## GPU offload (current KOMO deployment)

Agora decodes/scales/deinterlaces MPEG-2 in FFmpeg and its RTX 3060 encodes
H.264 with NVENC. Home-Pi uses the existing shared GStreamer V4L2 hardware
decoder **and** color converter, publishing 1024×576 RGBA frames at the
producer's 30 fps cadence. This avoids MPEG-2 decoding on the Pi.

Stop the direct HTTP viewer before starting the transcoder to release its
tuner. Do not restart the Unibus HDHomeRun proxy or disrupt FireTV playback.

On Agora, run in the `canvas-tv-komo4-transcode` user unit:

```sh
ffmpeg -nostdin -hide_banner -loglevel warning -rw_timeout 15000000 -threads 2 \
  -i http://192.168.2.74:5004/auto/v4.1 -map 0:v:0 -an \
  -vf 'yadif=mode=send_frame:deint=interlaced,scale=1024:576,fps=30,format=nv12,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709' \
  -c:v h264_nvenc -pix_fmt nv12 -profile:v main \
  -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv \
  -preset p1 -tune ull -rc cbr -b:v 4M -maxrate 4M -bufsize 256k \
  -g 30 -bf 0 -rc-lookahead 0 -zerolatency 1 -r 30 \
  -mpegts_flags resend_headers -muxdelay 0 -muxpreload 0 -flush_packets 1 \
  -f mpegts 'udp://192.168.2.74:55304?pkt_size=1316'
```

On Home-Pi, run in the `canvas-tv-komo4` user unit:

```sh
canvas-tv komo4 udp://192.168.2.74:55304
```

Both dimensions are macroblock-aligned and every RGBA row is 256-byte
aligned. Output color metadata is explicitly BT.709 rather than inheriting
unspecified decoder metadata, which can prevent V4L2 negotiation.
The tested CUVID hardware-resize path produced corrupt green/tiled video even
when decoded independently in software. It is deliberately **not** used by
this deployment. The reliable configuration offloads all MPEG-2 processing
to Agora and uses GPU encoding there; this is not an all-GPU/zero-copy transcode.
No B frames or encoder lookahead; one-second GOP; a one-frame leaky decode sink queue.
This is a trusted local-LAN UDP media path, not encrypted or authenticated
cross-site transport. No public ports or new discovery service are introduced.
The UDP argument is a receive endpoint; the shared planner uses its port.
Stop **both** units when finished; the Agora unit owns the tuner connection.
Both units remain transient and video-only. These settings do not constitute
a measured end-to-end latency guarantee.
