//! Verified immutable map assets and shared projection, never network fetching.
use fpl_gfx::{geo_map::Map, Rect};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

pub fn frame(root: &Path, map: &Map, width: f32, height: f32) -> Result<Value, String> {
    map.validate().map_err(|e| e.to_string())?;
    let plot = map
        .inner(Rect::new(0.0, 0.0, width, height))
        .map_err(|e| e.to_string())?;
    let Some(layer) = &map.basemap else {
        return Ok(json!({"tiles":[]}));
    };
    let tiles = layer.tiles.iter().map(|a| a.tile()).collect::<Vec<_>>();
    let placements = fpl_gfx::geo_map::raster::plan(
        fpl_gfx::geo_map::raster::cropped_extent(map.extent, plot).map_err(|e| e.to_string())?,
        plot,
        &tiles,
    )
    .map_err(|e| e.to_string())?;
    // Cropped-out manifest assets are not pending work. Otherwise a fully
    // loaded viewport recomposes and rehashes every second forever.
    let expected = placements.len();
    let mut loaded = Vec::new();
    for p in placements {
        let asset = &layer.tiles[p.tile_index];
        let path = root.join("assets").join(&asset.sha256);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if !metadata.is_file() || metadata.len() != asset.byte_size {
            return Err("Tile file type or size mismatch".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .map_err(|e| e.to_string())?
            .take(asset.byte_size + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 != asset.byte_size
            || bytes.len() < 24
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || &bytes[12..16] != b"IHDR"
            || bytes[16..20] != asset.width.to_be_bytes()
            || bytes[20..24] != asset.height.to_be_bytes()
            || format!("{:x}", Sha256::digest(&bytes)) != asset.sha256
        {
            return Err("Tile PNG identity mismatch".into());
        }
        loaded.push(json!({"sha256":asset.sha256,"path":path.to_string_lossy(),"rows":p.rows,"vertices":p.vertices}));
    }
    Ok(
        json!({"plot":[plot.x,plot.y,plot.right(),plot.bottom()],"attribution":layer.attribution,"expected":expected,"tiles":loaded}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_tiles_remain_loading_and_invalid_manifest_fails() {
        let mut map:Map=serde_json::from_value(json!({"extent":{"south_deg":46.8,"north_deg":48.7,"west_deg":-123.5,"east_deg":-121.5},"markers":[],"paths":[],"basemap":{"attribution":"© OpenStreetMap contributors","tiles":[{"zoom":9,"x":81,"y":178,"width":256,"height":256,"sha256":"a".repeat(64),"byte_size":100}]}})).unwrap();
        let root = std::env::temp_dir().join("canvas-nonexistent-map-asset-store");
        let value = frame(&root, &map, 800.0, 600.0).unwrap();
        assert_eq!(value["expected"], 1);
        assert_eq!(value["tiles"].as_array().unwrap().len(), 0);
        map.basemap.as_mut().unwrap().tiles[0].x = 0;
        map.basemap.as_mut().unwrap().tiles[0].y = 0;
        let cropped_out = frame(&root, &map, 800.0, 600.0).unwrap();
        assert_eq!(cropped_out["expected"], 0);
        map.basemap.as_mut().unwrap().tiles[0].sha256 = "../not-an-asset".into();
        assert!(frame(&root, &map, 800.0, 600.0).is_err());
    }
}
