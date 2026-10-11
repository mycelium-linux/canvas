//! Current-viewport cache delivery, not a tile downloader or offline archive.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, process::Command};

pub struct CachedViewport {
    pub layer: Value,
    pub payloads: Vec<Vec<u8>>,
}

fn target_zoom(longitude_span: f64, width: u32) -> u8 {
    ((f64::from(width) * 360.0 / (longitude_span * 256.0))
        .log2()
        .floor())
    .clamp(0.0, 20.0) as u8
}

pub fn cached(map: &Value, root: &Path) -> Result<CachedViewport, Box<dyn std::error::Error>> {
    let e = &map["extent"];
    let coordinate = |name: &str| e[name].as_f64().ok_or("missing map extent");
    let west = coordinate("west_deg")?;
    let east = coordinate("east_deg")?;
    let south = coordinate("south_deg")?;
    let north = coordinate("north_deg")?;
    if ![west, east, south, north].iter().all(|n| n.is_finite())
        || west >= east
        || south >= north
        || south < -85.0
        || north > 85.0
        || west < -180.0
        || east > 180.0
    {
        return Err("invalid cached viewport".into());
    }
    // Viewer sampling target is a bounded owner configuration, not a fixed XYZ
    // zoom. A missing detailed cache falls back to a coarser cached level.
    let width = std::env::var("PANOPTICON_TILE_TARGET_WIDTH")
        .ok()
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(1280);
    if !(256..=4096).contains(&width) {
        return Err("tile target width must be 256..4096".into());
    }
    let desired = target_zoom(east - west, width);
    for zoom in (0..=desired).rev() {
        let n = f64::from(1u32 << zoom);
        let x = |lon: f64| (((lon + 180.0) / 360.0 * n).floor() as u32).min((1u32 << zoom) - 1);
        let y = |lat: f64| {
            (((1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor()
                as u32)
                .min((1u32 << zoom) - 1)
        };
        if (x(east) - x(west) + 1) as u64 * (y(south) - y(north) + 1) as u64 > 64 {
            continue;
        }
        let mut assets = Vec::new();
        let mut payloads = Vec::new();
        for tx in x(west)..=x(east) {
            for ty in y(north)..=y(south) {
                let path = root.join(format!("{zoom}/{tx}/{ty}.tile"));
                let mut file = match std::fs::File::open(path) {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                let mut bytes = Vec::new();
                (&mut file).take(65537).read_to_end(&mut bytes)?;
                let sha = format!("{:x}", Sha256::digest(&bytes));
                verify(&json!({"sha256":sha,"byte_size":bytes.len()}), &bytes)?;
                assets.push(json!({"zoom":zoom,"x":tx,"y":ty,"width":256,"height":256,"sha256":sha,"byte_size":bytes.len()}));
                payloads.push(bytes);
            }
        }
        if assets.is_empty() {
            continue;
        }
        return Ok(CachedViewport {
            layer: json!({"attribution":"© OpenStreetMap contributors","tiles":assets}),
            payloads,
        });
    }
    Err("no current viewport tiles at a budgeted LOD in owner cache".into())
}

pub fn verify(asset: &Value, bytes: &[u8]) -> Result<(), String> {
    let sha = asset["sha256"].as_str().ok_or("missing tile digest")?;
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || bytes.len() < 24
        || bytes.len() > 65536
        || asset["byte_size"].as_u64() != Some(bytes.len() as u64)
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
        || bytes[16..20] != 256u32.to_be_bytes()
        || bytes[20..24] != 256u32.to_be_bytes()
        || format!("{:x}", Sha256::digest(bytes)) != sha
    {
        return Err("tile digest, size or PNG dimensions rejected".into());
    }
    Ok(())
}

pub fn provision(
    endpoint: &str,
    asset: &Value,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    verify(asset, bytes)?;
    let _: std::net::SocketAddr = endpoint.parse()?;
    let sha = asset["sha256"].as_str().ok_or("missing digest")?;
    let local = std::env::temp_dir().join(format!("canvas-tile-{sha}"));
    std::fs::write(&local, bytes)?;
    let staging = format!("/data/local/tmp/canvas-tile-{sha}");
    let run = |args: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("adb")
            .args(["-s", endpoint])
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "tile provisioning failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(())
    };
    run(&[
        "push",
        local.to_str().ok_or("invalid asset path")?,
        &staging,
    ])?;
    run(&[
        "shell",
        "run-as",
        "dev.fpl.canvas.tv",
        "mkdir",
        "-p",
        "files/assets",
    ])?;
    run(&[
        "shell",
        "run-as",
        "dev.fpl.canvas.tv",
        "cp",
        &staging,
        &format!("files/assets/{sha}"),
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lod_tracks_view_span_and_pixel_budget_instead_of_fixed_zoom() {
        assert_eq!(target_zoom(2.0, 1280), 9);
        assert_eq!(target_zoom(2.0, 2560), 10);
        assert_eq!(target_zoom(1.0, 1280), 10);
        assert_eq!(target_zoom(360.0, 256), 0);
        assert_eq!(target_zoom(1e-12, 4096), 20);
    }
    #[test]
    fn rejects_corruption_dimensions_and_traversal() {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(256u32.to_be_bytes());
        bytes.extend(256u32.to_be_bytes());
        let mut asset =
            json!({"sha256":format!("{:x}",Sha256::digest(&bytes)),"byte_size":bytes.len()});
        assert!(verify(&asset, &bytes).is_ok());
        bytes[23] = 0;
        bytes[19] = 1;
        assert!(verify(&asset, &bytes).is_err());
        asset["sha256"] = json!("../escape");
        assert!(verify(&asset, &bytes).is_err());
    }
}
