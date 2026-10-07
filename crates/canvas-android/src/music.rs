//! Platform adapter: shared catalog client and GFX geometry, no player implementation.
use canvas_feeds::music_library::{Album, Library};

pub fn library(origin: &str) -> Result<std::sync::Arc<Library>, String> {
    let url = url::Url::parse(origin).map_err(|_| "Invalid library origin")?;
    if url.host_str() != Some("127.0.0.1") || url.scheme() != "http" {
        return Err("Library requires a loopback authenticated bridge".into());
    }
    // One pooled client/runtime, not a new HTTP runtime for each focus/art request.
    type Cached = Option<(String, std::sync::Arc<Library>)>;
    static CLIENT: std::sync::OnceLock<std::sync::Mutex<Cached>> = std::sync::OnceLock::new();
    let mut cached = CLIENT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map_err(|_| "Library client unavailable")?;
    if let Some((key, client)) = cached.as_ref() {
        if key == origin {
            return Ok(client.clone());
        }
    }
    let client = std::sync::Arc::new(Library::new(origin, None)?);
    *cached = Some((origin.into(), client.clone()));
    Ok(client)
}

pub fn request(
    origin: &str,
    action: &str,
    artist: &str,
    album: &str,
    offset: usize,
) -> Result<String, String> {
    let library = library(origin)?;
    let selected = Album {
        artist: artist.into(),
        album: album.into(),
        year: None,
        tracks: 0,
    };
    let value = match action {
        "albums" => serde_json::json!({"albums":library.albums(offset)?}),
        "tracks" => serde_json::json!({"tracks":library.tracks(&selected)?}),
        "play" | "enqueue" => {
            serde_json::json!({"receipt":library.select(&selected, action == "play")?})
        }
        _ => return Err("Unknown library action".into()),
    };
    Ok(value.to_string())
}

pub fn geometry(width: u32, height: u32, count: usize) -> Result<String, String> {
    let shelf = fpl_gfx::media::Shelf::layout(
        fpl_gfx::Rect::new(0.0, 0.0, width as f32, height as f32),
        count,
    )
    .ok_or("Library viewport cannot fit this page")?;
    Ok(serde_json::json!({"cards":shelf.cards.iter().map(|card| serde_json::json!({"bounds":card.bounds,"artwork":card.artwork})).collect::<Vec<_>>(),"detail":shelf.detail}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_private_bridge_origins_are_accepted() {
        assert!(library("http://127.0.0.1:26601").is_ok());
        assert!(library("http://home-pi.local:6601").is_err());
        assert!(library("http://127.0.0.1:26601/arbitrary").is_err());
    }
    #[test]
    fn geometry_is_shared_and_bounded() {
        assert!(geometry(1920, 1080, 8).is_ok());
        assert!(geometry(640, 320, 24).is_err());
    }
    #[test]
    fn requests_share_a_single_client() {
        let a = library("http://127.0.0.1:26601").unwrap();
        let b = library("http://127.0.0.1:26601").unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b));
    }
}

#[cfg(target_os = "android")]
mod jni_api {
    use super::*;
    use jni::{
        JNIEnv,
        objects::{JClass, JString},
        sys::{jbyteArray, jint, jstring},
    };
    fn result(env: &mut JNIEnv, value: Result<String, String>) -> jstring {
        let raw = value.unwrap_or_else(|error| serde_json::json!({"error":error}).to_string());
        env.new_string(raw)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_LibraryView_accent(
        env: JNIEnv,
        _class: JClass,
        rgba: jni::objects::JByteArray,
    ) -> jint {
        let bytes = match env.get_array_length(&rgba) {
            Ok(size) if size <= 64 * 64 * 4 => env.convert_byte_array(&rgba).ok(),
            _ => None,
        };
        bytes
            .as_deref()
            .and_then(fpl_gfx::media::artwork_accent)
            .map(|color| {
                let [r, g, b, a] = color.to_srgb8();
                u32::from_be_bytes([a, r, g, b]) as jint
            })
            .unwrap_or(0)
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_LibraryView_request(
        mut env: JNIEnv,
        _class: JClass,
        origin: JString,
        action: JString,
        artist: JString,
        album: JString,
        offset: jint,
    ) -> jstring {
        let value = (|| {
            let origin: String = env.get_string(&origin).map_err(|e| e.to_string())?.into();
            let action: String = env.get_string(&action).map_err(|e| e.to_string())?.into();
            let artist: String = env.get_string(&artist).map_err(|e| e.to_string())?.into();
            let album: String = env.get_string(&album).map_err(|e| e.to_string())?.into();
            super::request(
                &origin,
                &action,
                &artist,
                &album,
                offset.try_into().map_err(|_| "Invalid offset")?,
            )
        })();
        result(&mut env, value)
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_LibraryView_layout(
        mut env: JNIEnv,
        _class: JClass,
        width: jint,
        height: jint,
        count: jint,
    ) -> jstring {
        let value = (|| {
            super::geometry(
                width.try_into().map_err(|_| "Invalid width")?,
                height.try_into().map_err(|_| "Invalid height")?,
                count.try_into().map_err(|_| "Invalid count")?,
            )
        })();
        result(&mut env, value)
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_LibraryView_artwork(
        mut env: JNIEnv,
        _class: JClass,
        origin: JString,
        artist: JString,
        album: JString,
    ) -> jbyteArray {
        let value = (|| -> Result<Vec<u8>, String> {
            let origin: String = env.get_string(&origin).map_err(|e| e.to_string())?.into();
            let artist: String = env.get_string(&artist).map_err(|e| e.to_string())?.into();
            let album: String = env.get_string(&album).map_err(|e| e.to_string())?.into();
            library(&origin)?
                .artwork(&Album {
                    artist,
                    album,
                    year: None,
                    tracks: 0,
                })
                .map(|art| art.unwrap_or_default())
        })();
        match value {
            Ok(bytes) => env
                .byte_array_from_slice(&bytes)
                .map(|value| value.into_raw())
                .unwrap_or(std::ptr::null_mut()),
            Err(error) => {
                let _ = env.throw_new("java/io/IOException", error);
                std::ptr::null_mut()
            }
        }
    }
}
