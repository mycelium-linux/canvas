//! Immutable digest assets become a retained GPU image; no network fetches.
use fpl_gfx::{Rect, geo_map::Map};
use iced::widget::image::Handle;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path};
#[cfg(test)]
#[path = "map_raster_tests.rs"]
mod tests;

pub fn retained(map: &Map, width: u32, height: u32, root: &Path) -> Result<Handle, String> {
    type Entries = std::collections::VecDeque<(String, Handle)>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Entries>> = std::sync::OnceLock::new();
    let style = std::env::var("CANVAS_MAP_STYLE").unwrap_or_else(|_| "source".into());
    if style != "source" && style != "tactical" {
        return Err("CANVAS_MAP_STYLE must be source or tactical".into());
    }
    let key = format!(
        "{}:{width}:{height}:{style}:{}",
        root.display(),
        serde_json::to_string(&(map.extent, &map.basemap)).map_err(|e| e.to_string())?
    );
    let cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Map cache poisoned")?;
    if let Some((_, handle)) = cache.iter().find(|(entry, _)| entry == &key) {
        return Ok(handle.clone());
    }
    drop(cache);
    let handle = compose_styled(map, width, height, root, style == "tactical")?;
    let mut cache = CACHE
        .get()
        .unwrap()
        .lock()
        .map_err(|_| "Map cache poisoned")?;
    if let Some((_, handle)) = cache.iter().find(|(entry, _)| entry == &key) {
        return Ok(handle.clone());
    }
    if cache.len() == 4 {
        cache.pop_front();
    }
    cache.push_back((key, handle.clone()));
    Ok(handle)
}

#[cfg(test)]
fn compose(map: &Map, width: u32, height: u32, root: &Path) -> Result<Handle, String> {
    compose_styled(map, width, height, root, false)
}

fn compose_styled(
    map: &Map,
    width: u32,
    height: u32,
    root: &Path,
    tactical: bool,
) -> Result<Handle, String> {
    map.validate().map_err(|e| e.to_string())?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 4_194_304 {
        return Err("Map image exceeds pixel budget".into());
    }
    let plot = map
        .inner(Rect::new(0.0, 0.0, width as f32, height as f32))
        .map_err(|e| e.to_string())?;
    let extent =
        fpl_gfx::geo_map::raster::cropped_extent(map.extent, plot).map_err(|e| e.to_string())?;
    let layer = map.basemap.as_ref().ok_or("No map assets")?;
    let mut tiles = BTreeMap::new();
    for asset in &layer.tiles {
        let path = root.join(&asset.sha256);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|e| format!("Map asset unavailable: {e}"))?;
        if !metadata.is_file() || metadata.len() != asset.byte_size {
            return Err("Map asset size/type mismatch".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(asset.byte_size + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != asset.sha256 {
            return Err("Map asset digest mismatch".into());
        }
        if bytes.len() < 24
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || &bytes[12..16] != b"IHDR"
            || bytes[16..20] != 256u32.to_be_bytes()
            || bytes[20..24] != 256u32.to_be_bytes()
        {
            return Err("Map PNG dimensions/signature rejected".into());
        }
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        if decoded.width() != 256 || decoded.height() != 256 {
            return Err("Map asset dimensions mismatch".into());
        }
        tiles.insert((asset.zoom, asset.x, asset.y), decoded);
    }
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let longitude = (extent.east_deg - extent.west_deg).rem_euclid(360.0);
    let levels: std::collections::BTreeSet<_> = layer.tiles.iter().map(|a| a.zoom).collect();
    for y in plot.y.ceil() as u32..(plot.bottom().floor() as u32).min(height) {
        let latitude = extent.north_deg
            - (f64::from(y) + 0.5 - f64::from(plot.y)) / f64::from(plot.height)
                * (extent.north_deg - extent.south_deg);
        let world_y = (1.0 - latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
        for x in plot.x.ceil() as u32..(plot.right().floor() as u32).min(width) {
            let longitude = extent.west_deg
                + (f64::from(x) + 0.5 - f64::from(plot.x)) / f64::from(plot.width) * longitude;
            let world_x = ((longitude + 180.0) / 360.0).rem_euclid(1.0);
            for zoom in levels.iter().rev() {
                let n = f64::from(1u32 << zoom);
                let tx = world_x * n;
                let ty = world_y * n;
                if let Some(tile) = tiles.get(&(*zoom, tx.floor() as u32, ty.floor() as u32)) {
                    let pixel =
                        tile.get_pixel((tx.fract() * 256.0) as u32, (ty.fract() * 256.0) as u32);
                    let index = ((y as usize) * (width as usize) + x as usize) * 4;
                    let color = if tactical {
                        fpl_gfx::geo_map::raster::tactical_pixel(pixel.0)
                    } else {
                        pixel.0
                    };
                    rgba[index..index + 4].copy_from_slice(&color);
                    break;
                }
            }
        }
    }
    Ok(Handle::from_rgba(width, height, rgba))
}
