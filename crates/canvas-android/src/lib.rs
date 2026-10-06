//! Android platform shell. Rust owns command admission; Java owns the surface.
use std::net::IpAddr;
pub const CONTROL_PORT: u16 = 27841;

#[cfg(feature = "shared-workspaces")]
pub mod assets;
#[cfg(feature = "android-native-video")]
pub mod background;
#[cfg(feature = "android-native-video")]
pub mod clock;
#[cfg(feature = "android-native-video")]
pub mod signals;
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
