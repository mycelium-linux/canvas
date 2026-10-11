//! Android platform shell. Rust owns command admission; Java owns the surface.
use std::net::IpAddr;
pub const CONTROL_PORT: u16 = 27841;

#[cfg(all(test, feature = "android-native-video"))]
mod chrome_tests {
    #[test]
    fn preserves_shared_state_and_rejects_bad_frames() {
        let selected = super::chrome_records(100.0, 32.0, 1, false).unwrap();
        let focus = super::chrome_records(100.0, 32.0, 2, false).unwrap();
        assert_eq!(selected.len(), 10);
        assert_eq!(selected[6], fpl_gfx::ThemeRole::SurfaceRaised as u32 as f32);
        assert_eq!(focus[5], selected[5] * 2.0);
        assert_eq!(focus[8], fpl_gfx::ThemeRole::Text as u32 as f32);
        let disabled = super::chrome_records(100.0, 32.0, 15, false).unwrap();
        assert_eq!(disabled[8], fpl_gfx::ThemeRole::TextMuted as u32 as f32);
        assert_eq!(disabled[9], 0.45);
        let quiet = super::chrome_records_with_appearance(100.0, 32.0, 0, false, true).unwrap();
        assert_eq!(quiet[5], 0.0);
        assert_eq!(quiet[9], 0.0);
        let bar = super::chrome_records(100.0, 32.0, 0, true).unwrap();
        assert_eq!(bar[6], fpl_gfx::ThemeRole::Surface as u32 as f32);
        assert!(super::chrome_records(f32::NAN, 32.0, 0, false).is_err());
        assert!(super::chrome_records(100.0, 32.0, 16, false).is_err());
    }
}

/// Bounded raster adapter for the shared GFX shell scene.
#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_ShellChrome_nativeMetrics(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
) -> jni::sys::jintArray {
    let values = fpl_gfx::chrome::metrics().map(|value| value as i32);
    let Ok(array) = env.new_int_array(values.len() as i32) else { return std::ptr::null_mut(); };
    if env.set_int_array_region(&array, 0, &values).is_err() { return std::ptr::null_mut(); }
    array.into_raw()
}

/// Bounded raster adapter for the shared GFX shell scene.
#[cfg(feature = "android-native-video")]
pub fn chrome_records(
    width: f32,
    height: f32,
    state: i32,
    bar: bool,
) -> Result<Vec<f32>, &'static str> {
    chrome_records_with_appearance(width, height, state, bar, false)
}

#[cfg(feature = "android-native-video")]
fn chrome_records_with_appearance(
    width: f32,
    height: f32,
    state: i32,
    bar: bool,
    quiet: bool,
) -> Result<Vec<f32>, &'static str> {
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > 16384.0
        || height > 16384.0
        || !(0..=15).contains(&state)
    {
        return Err("Invalid chrome frame");
    }
    let bounds = fpl_gfx::Rect::new(0.0, 0.0, width, height);
    let (primitive, ink, opacity) = if bar {
        let mut scene = fpl_gfx::Scene::with_capacity(1);
        fpl_gfx::chrome::bar(&mut scene, bounds);
        (
            *scene.iter().next().ok_or("Invalid bar frame")?,
            fpl_gfx::ThemeRole::Text,
            1.0,
        )
    } else {
        let metrics = canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .metrics();
        let control = fpl_gfx::chrome::control_visual(
            bounds,
            fpl_gfx::ShapeTheme {
                radius_small: metrics.radius_sm,
                radius_large: metrics.radius_lg,
                outline_width: 1.0,
            },
            fpl_gfx::chrome::ControlState {
                selected: state & 1 != 0,
                focused: state & 2 != 0,
                pressed: state & 4 != 0,
                disabled: state & 8 != 0,
            },
            if quiet {
                fpl_gfx::chrome::ControlAppearance::Quiet
            } else {
                fpl_gfx::chrome::ControlAppearance::Standard
            },
        )
        .ok_or("Invalid control frame")?;
        (control.face, control.ink, control.face_opacity)
    };
    Ok(vec![
        primitive.rect.x,
        primitive.rect.y,
        primitive.rect.width,
        primitive.rect.height,
        primitive.radius,
        primitive.outline_width,
        primitive.fill as u32 as f32,
        primitive.outline as u32 as f32,
        ink as u32 as f32,
        opacity,
    ])
}

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_ShellChrome_nativeFrame(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jfloat,
    height: jni::sys::jfloat,
    state: jni::sys::jint,
    bar: jni::sys::jboolean,
    quiet: jni::sys::jboolean,
) -> jni::sys::jfloatArray {
    let records = match chrome_records_with_appearance(width, height, state, bar != 0, quiet != 0) {
        Ok(records) => records,
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            return std::ptr::null_mut();
        }
    };
    let Ok(array) = env.new_float_array(records.len() as i32) else {
        return std::ptr::null_mut();
    };
    if env.set_float_array_region(&array, 0, &records).is_err() {
        return std::ptr::null_mut();
    }
    array.into_raw()
}

#[cfg(feature = "shared-workspaces")]
pub mod assets;
#[cfg(feature = "android-native-video")]
pub mod background;
#[cfg(feature = "android-native-video")]
pub mod clock;
#[cfg(feature = "android-native-video")]
pub mod boids;
#[cfg(feature = "android-native-video")]
pub mod signals;
#[cfg(feature = "android-native-video")]
pub mod music;
#[cfg(any(feature = "android-native-video", feature = "android-tuner-control"))]
pub mod tuner;
#[cfg(feature = "shared-workspaces")]
pub mod geometry;
#[cfg(all(target_os = "android", feature = "android-native-video"))]
mod native_control;
#[cfg(feature = "shared-workspaces")]
pub mod workspaces;

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeVideoFrame(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
    windowed: jni::sys::jboolean,
) -> jni::sys::jintArray {
    let frame = match (u32::try_from(width), u32::try_from(height)) {
        (Ok(width), Ok(height)) => geometry::video_frame(width, height, windowed != 0),
        _ => Err("Invalid output extent".into()),
    };
    let frame = match frame {
        Ok(frame) => frame,
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            return std::ptr::null_mut();
        }
    };
    let array = match env.new_int_array(4) {
        Ok(array) => array,
        Err(_) => return std::ptr::null_mut(),
    };
    if env
        .set_int_array_region(
            &array,
            0,
            &[frame.x, frame.y, frame.width as i32, frame.height as i32],
        )
        .is_err()
    {
        return std::ptr::null_mut();
    }
    array.into_raw()
}

/// Source-local audio gain; this never changes Android's master/media volume.
pub fn surface_gain(muted: bool) -> f32 {
    if muted { 0.0 } else { 1.0 }
}

#[cfg(all(target_os = "android", feature = "android-native-video"))]
static SURFACE_MUTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSetMuted(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    muted: jni::sys::jboolean,
) -> jni::sys::jfloat {
    let muted = muted != 0;
    SURFACE_MUTED.store(muted, std::sync::atomic::Ordering::Relaxed);
    surface_gain(muted)
}

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeIsMuted(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
) -> jni::sys::jboolean {
    SURFACE_MUTED.load(std::sync::atomic::Ordering::Relaxed) as jni::sys::jboolean
}

/// Initial playback accepts explicit private-LAN HTTP endpoints only. This is
/// not discovery, pairing, or permission to expose an owner control token.
pub fn validate_stream(input: &str) -> Result<(), &'static str> {
    let url = url::Url::parse(input).map_err(|_| "Invalid stream URL")?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Expected a private-LAN HTTP stream without credentials");
    }
    let address: IpAddr = url
        .host_str()
        .ok_or("Missing host")?
        .parse()
        .map_err(|_| "Use an explicit private IPv4 address")?;
    match address {
        IpAddr::V4(ip) if ip.is_private() => Ok(()),
        _ => Err("Stream must be on a private IPv4 network"),
    }
}

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeValidate(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    input: jni::objects::JString,
) -> jni::sys::jstring {
    let result = env
        .get_string(&input)
        .map(String::from)
        .map_err(|_| "Could not read stream URL")
        .and_then(|s| validate_stream(&s));
    match env.new_string(result.err().unwrap_or("")) {
        Ok(message) => message.into_raw(),
        Err(_) => std::ptr::null_mut(), // JVM allocation exception remains pending.
    }
}

#[cfg(all(target_os = "android", feature = "android-native-video"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_MattedVideoView_nativeShaders(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
) -> jni::sys::jstring {
    let shaders = serde_json::json!({
        "vertex": fpl_gfx::video_matte::GLES_VERTEX,
        "fragment": fpl_gfx::video_matte::GLES_FRAGMENT,
    });
    match env.new_string(shaders.to_string()) {
        Ok(message) => message.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mute_is_source_local_and_reversible() {
        assert_eq!(surface_gain(true), 0.0);
        assert_eq!(surface_gain(false), 1.0);
    }
    #[test]
    fn accepts_explicit_lan_stream() {
        assert!(validate_stream("http://192.168.2.74:5004/auto/v4.1").is_ok());
    }
    #[test]
    fn rejects_untrusted_destinations_and_credentials() {
        for value in [
            "https://192.168.2.74/x",
            "http://example.com/x",
            "http://8.8.8.8/x",
            "http://127.0.0.1/x",
            "http://user:pass@192.168.2.74/x",
            "file:///etc/passwd",
            "http://192.168.2.74/x#fragment",
        ] {
            assert!(validate_stream(value).is_err(), "{value}");
        }
    }
}
