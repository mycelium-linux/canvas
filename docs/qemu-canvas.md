# QEMU display in Canvas

`canvas-qemu-stream` presents a QEMU guest display as an ordinary Canvas
`live_video` surface. It reads pixels from QEMU's own RFB/VNC display, not from
an Agora One desktop window. QMP is a separate control/discovery channel and
does not carry video or input.

```text
QEMU RFB :5901 -> rfbsrc -> NVENC H.264 -> UDP MPEG-TS -> canvas-media-edge
QEMU QMP socket ---------------------------------------> validation/control
Unibus media.video_stream.v1 --------------------------> discovery/lease
```

The configured width and height drive both GStreamer's output caps and the
Unibus announcement. `canvas-media-edge` publishes its decoded frame-size
sidecar before the first frame. A geometry change therefore requires a new
pipeline/lease generation rather than interpreting old bytes with new bounds.

## QEMU

Start or amend a guest with owner-local QMP and VNC sockets. Bind VNC only to
loopback; the adapter runs on the same host.

```sh
qemu-system-x86_64 \
  -enable-kvm \
  -qmp unix:/run/user/1000/qemu/demo.qmp,server=on,wait=off \
  -display vnc=127.0.0.1:1 \
  ...
```

`:1` means TCP port 5901. QMP `query-vnc` must report the configured port or
the adapter fails before publishing a stream.

## Agora One adapter

Copy
[`qemu-stream.example.json`](../config/deploy/agora-one/qemu-stream.example.json),
then set the media-plane destination and the existing Agora One Unibus tunnel.
The receiver address is deployment-specific; never advertise the loopback RFB
endpoint to Canvas.

```sh
cargo build --release -p canvas-media-edge --bin canvas-qemu-stream
target/release/canvas-qemu-stream /path/to/qemu-stream.json
```

The required Agora One GStreamer elements are `rfbsrc`, `cudaupload`,
`nvh264enc`, `h264parse`, `mpegtsmux`, and `udpsink`. The pipeline uses one-frame
leaky queues, no B-frames or lookahead, a one-second GOP, and NVENC zero-latency
mode. A pipeline exit is fatal and visibly withdraws when the 30-second stream
lease expires.

## Canvas

Ensure `canvas-media-edge` is running on the intended display node. Install or
sync [`qemu.json`](../config/workspaces/tv/qemu.json), then select it:

```sh
canvasctl workspace switch qemu
```

The demo document contains one `live_video` surface with source id
`agora-qemu-demo`. The same source can be tiled into a larger workspace without
changing the producer.

## Input and control boundary

This first slice is deliberately view-only. Future keyboard/pointer support
should translate Canvas's generation-bound normalized input into QEMU input
commands over a separately authorized QMP session. Video workers must never
receive arbitrary QMP commands, guest paths, or producer-provided pipelines.

## Troubleshooting

```sh
# Verify QEMU's display/control sockets locally on Agora One
ss -ltn | grep 5901
test -S /run/user/1000/qemu/demo.qmp

# Verify host codecs
gst-inspect-1.0 rfbsrc nvh264enc cudaupload

# Verify the viewer lease and decoder
journalctl --user -u canvas-qemu-stream -f
systemctl status canvas-media-edge
```

If the image is diagonal, stretched, or periodically walks through memory,
stop the adapter: the producer caps, announcement geometry, and receiver frame
size do not describe the same generation. Do not compensate by guessing a
stride. Fix the declared geometry and restart the generation.
