# Stream video mattes

Android stream surfaces accept an optional `matte` owned and validated by
`fpl_gfx::video_matte::VideoMatte`. Existing streams omit it and retain opaque
playback. Example:

```json
{"kind":"stream","url":"http://host/replay.m3u8","matte":{
  "source_size":[1280,960],
  "regions":[{"bounds":[0,0,0.625,1],"color":[0,0,0],"cutoff":0.01,"feather":0.02}]
}}
```

Bounds are normalized source-space `[x,y,width,height]`, starting at the top
left, independent of tile scaling and letterboxing. Regions combine by minimum
opacity. Colors describe decoded sRGB bytes; cutoff and feather use normalized
maximum-channel distance. The Rust owner bounds region count, dimensions and
numeric ranges and supplies both a CPU oracle and the GLES shader. No shader
copy is stored in the Android adapter.

`MattedVideoView` is a platform shim: MediaPlayer decodes into an external OES
texture; the GFX shader emits premultiplied RGBA to a transparent EGL TextureView.
See the Android [TextureView contract](https://developer.android.com/reference/android/view/TextureView).
The view preserves aspect ratio, checks actual decoded dimensions, and releases
decoder, surfaces and EGL resources on errors or disposal. EGL display ownership
is reference counted across tiles. Rendering follows decoder frames rather than
polling. Local tile mute controls source gain. Remote primary-player audio
acknowledgements are not implemented for these independent tiles.

Color keying approximates silhouettes: objects matching the selected background
inside a region can become transparent, and compressed edges can retain seams.
Prefer exact separate alpha when a renderer produces it. Exclude PDF/text regions
that must remain opaque. This capability is implemented on Android; it does not
establish support on other Canvas hosts.

To enable, upsert the existing stream surface with a validated matte through the
revision-checked workspace endpoint. Test with actual decoded frames and the
target device. To revert, remove `matte` using another revision-checked upsert;
the stream returns to VideoView. Remove the field before installing an older
application that cannot deserialize it. Do not clear app data.
