//! Platform adapter: shared catalog client and GFX geometry, no player implementation.
use canvas_feeds::music_library::{Album, Library};

pub fn library(origin: &str) -> Result<std::sync::Arc<Library>, String> {
    let url = url::Url::parse(origin).map_err(|_| "Invalid library origin")?;
    if url.host_str() != Some("127.0.0.1") || url.scheme() != "http" {
        return Err("Library requires a loopback authenticated bridge".into());
    }
    // One pooled client/runtime, not a new HTTP runtime for each focus/art request.
    type Cached = std::collections::BTreeMap<String, std::sync::Arc<Library>>;
    static CLIENT: std::sync::OnceLock<std::sync::Mutex<Cached>> = std::sync::OnceLock::new();
    let mut cached = CLIENT
        .get_or_init(|| std::sync::Mutex::new(Cached::new()))
        .lock()
        .map_err(|_| "Library client unavailable")?;
    if let Some(client) = cached.get(origin) {
        return Ok(client.clone());
    }
    if cached.len() >= 4 {
        return Err("Too many configured music owner origins".into());
    }
    let client = std::sync::Arc::new(Library::new(origin, None)?);
    cached.insert(origin.into(), client.clone());
    Ok(client)
}

pub fn controls(origin: &str, action: &str, id: &str) -> Result<String, String> {
    let client = library(origin)?;
    let result = match action {
        "player.status" => serde_json::json!({"player":client.player_status()?}),
        "inputs" => serde_json::json!({"inputs":client.reference_inputs()?}),
        "mute" | "solo" => serde_json::json!({"inputs":client.reference_control(id,action)?}),
        command if command.starts_with("player.") => {
            serde_json::json!({"player":client.transport(&command[7..])?})
        }
        _ => return Err("Unsupported playback control".into()),
    };
    Ok(result.to_string())
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
        "albums" => serde_json::json!({"albums":library.albums_by_artist(offset, artist)?}),
        "artists" => serde_json::json!({"artists":library.artists(offset)?}),
        "playlists" => serde_json::json!({"playlists":library.playlists(offset)?}),
        "playlist.tracks" => serde_json::json!({"tracks":library.playlist_tracks(artist,album)?}),
        "tracks" => serde_json::json!({"tracks":library.tracks(&selected)?}),
        "play" | "enqueue" => {
            serde_json::json!({"receipt":library.select(&selected, action == "play")?})
        }
        _ => return Err("Unknown library action".into()),
    };
    Ok(value.to_string())
}

pub fn geometry(width: u32, height: u32, count: usize) -> Result<String, String> {
    if count > fpl_gfx::media::MAX_CARDS {
        return Err("Too many shelf cards".into());
    }
    let bounds = fpl_gfx::Rect::new(0.0, 0.0, width as f32, height as f32);
    let shelf =
        fpl_gfx::media::Shelf::layout(bounds, 0).ok_or("Library viewport cannot fit this page")?;
    let grid = fpl_gfx::media::RecordCrate::layout(
        fpl_gfx::Rect::new(
            0.0,
            0.0,
            shelf.detail.x - fpl_gfx::media::DETAIL_GAP,
            height as f32,
        ),
        count,
    )
    .ok_or("Record crate cannot fit beside detail")?;
    Ok(serde_json::json!({"cards":grid.cards.iter().map(|card| serde_json::json!({"bounds":card.bounds,"sleeve":card.sleeve,"record":card.record,"artwork":card.sleeve})).collect::<Vec<_>>(),"detail":shelf.detail}).to_string())
}

pub fn crate_geometry(width: u32, height: u32, count: usize) -> Result<String, String> {
    let grid = fpl_gfx::media::RecordCrate::layout(
        fpl_gfx::Rect::new(0.0, 0.0, width as f32, height as f32),
        count,
    )
    .ok_or("Record crate cannot fit this viewport")?;
    Ok(serde_json::json!({"columns":grid.columns,"capacity":grid.capacity,"cards":grid.cards.iter().map(|card|serde_json::json!({"bounds":card.bounds,"sleeve":card.sleeve,"record":card.record})).collect::<Vec<_>>()} ).to_string())
}

/// Quiet deterministic paper-brush UI cue, independent of the studio audio engine.
pub fn record_selection_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    const FRAMES: u32 = 882;
    let mut bytes = Vec::with_capacity(44 + FRAMES as usize * 2);
    bytes.extend(b"RIFF");
    bytes.extend((36 + FRAMES * 2).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((FRAMES * 2).to_le_bytes());
    let mut seed = 0x7a31u32;
    let mut low = 0.0f32;
    for index in 0..FRAMES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as f32 / 32767.5) - 1.0;
        low += (noise - low) * 0.2;
        let t = index as f32 / (FRAMES - 1) as f32;
        let envelope = (std::f32::consts::PI * t).sin().powi(2);
        let sample = (low * envelope * 0.18 * i16::MAX as f32) as i16;
        bytes.extend(sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_cue_is_bounded_quiet_and_deterministic() {
        let bytes = record_selection_wav();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 882 * 2);
        assert_eq!(bytes, record_selection_wav());
        assert!(
            bytes[44..]
                .chunks_exact(2)
                .all(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() < 6000)
        );
    }
    #[test]
    fn crate_geometry_paginates_without_overflow() {
        let grid: serde_json::Value =
            serde_json::from_str(&crate_geometry(960, 540, 24).unwrap()).unwrap();
        assert!(grid["cards"].as_array().unwrap().len() < 24);
        assert!(grid["columns"].as_u64().unwrap() > 1);
        assert!(crate_geometry(960, 540, 25).is_err());
    }
    #[test]
    fn only_private_bridge_origins_are_accepted() {
        assert!(library("http://127.0.0.1:26601").is_ok());
        assert!(library("http://home-pi.local:6601").is_err());
        assert!(library("http://127.0.0.1:26601/arbitrary").is_err());
    }
    #[test]
    fn geometry_is_shared_and_bounded() {
        assert!(geometry(1920, 1080, 8).is_ok());
        let compact: serde_json::Value =
            serde_json::from_str(&geometry(960, 540, 6).unwrap()).unwrap();
        assert!(!compact["cards"].as_array().unwrap().is_empty());
        assert!(geometry(960, 540, 25).is_err());
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
    pub extern "system" fn Java_dev_fpl_canvas_RecordSounds_selectionWav(
        env: JNIEnv,
        _class: JClass,
    ) -> jbyteArray {
        env.byte_array_from_slice(&super::record_selection_wav())
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_LibraryView_crateLayout(
        mut env: JNIEnv,
        _class: JClass,
        width: jint,
        height: jint,
        count: jint,
    ) -> jstring {
        let value = (|| {
            super::crate_geometry(
                width.try_into().map_err(|_| "Invalid width")?,
                height.try_into().map_err(|_| "Invalid height")?,
                count.try_into().map_err(|_| "Invalid count")?,
            )
        })();
        result(&mut env, value)
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_RecordTile_vinylRadii(
        env: JNIEnv,
        _class: JClass,
        diameter: jint,
    ) -> jni::sys::jfloatArray {
        let Some(values) = fpl_gfx::media::vinyl_radii(diameter as f32) else {
            return std::ptr::null_mut();
        };
        let Ok(array) = env.new_float_array(values.len() as i32) else {
            return std::ptr::null_mut();
        };
        if env.set_float_array_region(&array, 0, &values).is_err() {
            return std::ptr::null_mut();
        }
        array.into_raw()
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_TvMotion_curve(
        env: JNIEnv,
        _class: JClass,
    ) -> jni::sys::jfloatArray {
        let mut tween = fpl_gfx::motion::Tween::new(0.0).expect("finite motion origin");
        tween.retarget(1.0, 1.0);
        let values: Vec<f32> = (0..=60)
            .map(|index| {
                let mut sample = tween;
                sample.advance(index as f32 / 60.0)
            })
            .collect();
        let Ok(array) = env.new_float_array(values.len() as i32) else {
            return std::ptr::null_mut();
        };
        if env.set_float_array_region(&array, 0, &values).is_err() {
            return std::ptr::null_mut();
        }
        array.into_raw()
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_fpl_canvas_ReferenceControls_request(
        mut env: JNIEnv,
        _class: JClass,
        origin: JString,
        action: JString,
        id: JString,
    ) -> jstring {
        let value = (|| {
            let origin: String = env.get_string(&origin).map_err(|e| e.to_string())?.into();
            let action: String = env.get_string(&action).map_err(|e| e.to_string())?.into();
            let id: String = env.get_string(&id).map_err(|e| e.to_string())?.into();
            super::controls(&origin, &action, &id)
        })();
        result(&mut env, value)
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
