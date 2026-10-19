//! Canvas transport/composition around Galileo's tile selection and disk cache.
use anyhow::{Context, Result, ensure};
use galileo::{
    tile_cache::TileCache,
    tile_lod::{RequestReason, TileId, TilePolicy, TileView, plan_tiles},
};
use image::RgbaImage;
use std::{
    collections::VecDeque,
    process::Command,
    sync::{Arc, Mutex, mpsc},
};

pub struct Raster {
    pub bounds: [f64; 4],
    pub image: RgbaImage,
    pub zoom: u8,
    pub tiles: usize,
}

pub struct Stream {
    pub requests: mpsc::SyncSender<TileView>,
    pub results: mpsc::Receiver<Result<Raster>>,
}

pub fn start(template: String) -> Stream {
    let (sender, requests) = mpsc::sync_channel(1);
    let (results, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let cache = TileCache::from_environment(&template);
        while let Ok(view) = requests.recv() {
            let raster = load(view, &template, cache.as_ref());
            if results.send(raster).is_err() {
                break;
            }
        }
    });
    Stream {
        requests: sender,
        results: receiver,
    }
}

fn rectangle(view: TileView) -> Result<(u8, u32, u32, u32, u32)> {
    for maximum_zoom in (0..=18).rev() {
        let requests = plan_tiles(
            view,
            &[],
            TilePolicy {
                maximum_zoom,
                request_budget: 512,
                ..Default::default()
            },
        );
        let visible: Vec<_> = requests
            .iter()
            .filter(|r| r.reason == RequestReason::Visible)
            .map(|r| r.tile)
            .collect();
        let zoom = visible
            .first()
            .context("tile planner returned no visible tiles")?
            .zoom;
        let min_x = visible.iter().map(|t| t.x).min().unwrap();
        let max_x = visible.iter().map(|t| t.x).max().unwrap();
        let min_y = visible.iter().map(|t| t.y).min().unwrap();
        let max_y = visible.iter().map(|t| t.y).max().unwrap();
        if u64::from(max_x - min_x + 1) * u64::from(max_y - min_y + 1) <= 64
            && max_x - min_x < 16
            && max_y - min_y < 16
        {
            return Ok((zoom, min_x, min_y, max_x, max_y));
        }
    }
    anyhow::bail!("visible tile rectangle exceeds budget")
}

fn load(view: TileView, template: &str, cache: Option<&TileCache>) -> Result<Raster> {
    let (zoom, min_x, min_y, max_x, max_y) = rectangle(view)?;
    let jobs = Arc::new(Mutex::new(VecDeque::new()));
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            jobs.lock().unwrap().push_back(TileId { zoom, x, y });
        }
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let jobs = Arc::clone(&jobs);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let Some(tile) = jobs.lock().unwrap().pop_front() else {
                        break;
                    };
                    if sender
                        .send((tile, load_tile(tile, template, cache)))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    drop(sender);
    let mut image = RgbaImage::new((max_x - min_x + 1) * 256, (max_y - min_y + 1) * 256);
    let mut count = 0;
    for (tile, result) in receiver {
        let tile_image = result?;
        image::imageops::replace(
            &mut image,
            &tile_image,
            i64::from((tile.x - min_x) * 256),
            i64::from((tile.y - min_y) * 256),
        );
        count += 1;
    }
    let n = f64::from(1_u32 << zoom);
    let latitude =
        |y: f64| ((std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan()).to_degrees();
    Ok(Raster {
        bounds: [
            f64::from(min_x) / n * 360.0 - 180.0,
            latitude(f64::from(max_y + 1)),
            f64::from(max_x + 1) / n * 360.0 - 180.0,
            latitude(f64::from(min_y)),
        ],
        image,
        zoom,
        tiles: count,
    })
}

fn load_tile(tile: TileId, template: &str, cache: Option<&TileCache>) -> Result<RgbaImage> {
    if let Some(cache) = cache {
        if let Some(bytes) = cache.read(tile).context("read Galileo tile cache")? {
            if let Ok(image) = decode_tile(&bytes) {
                return Ok(image);
            }
            eprintln!("tiles: rejected corrupt cached tile {tile:?}; fetching again");
        }
    }
    let url = template
        .replace("{z}", &tile.zoom.to_string())
        .replace("{x}", &tile.x.to_string())
        .replace("{y}", &tile.y.to_string());
    ensure!(
        url.starts_with("https://"),
        "imagery provider must use HTTPS"
    );
    let output = Command::new("/usr/bin/curl")
        .args([
            "--silent",
            "--fail",
            "--max-time",
            "5",
            "--max-filesize",
            "2097152",
            "--proto",
            "=https",
            "--user-agent",
            "Canvas-Panopticon/1.0",
            &url,
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "imagery request failed for {tile:?}"
    );
    let image = decode_tile(&output.stdout)?;
    if let Some(cache) = cache {
        cache
            .write(tile, &output.stdout)
            .context("write Galileo tile cache")?;
    }
    Ok(image)
}

fn decode_tile(bytes: &[u8]) -> Result<RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    limits.max_alloc = Some(1_048_576);
    reader.limits(limits);
    let image = reader.decode()?;
    ensure!(
        image.width() == 256 && image.height() == 256,
        "provider tile must be 256x256"
    );
    Ok(image.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn puget_and_puerto_rico_select_bounded_detail() {
        for (latitude, longitude) in [(47.6, -122.3), (18.4, -66.1)] {
            let (zoom, x0, y0, x1, y1) = rectangle(TileView {
                latitude,
                longitude,
                meters_per_pixel: 150.0,
                viewport_width_px: 1280.0,
                viewport_height_px: 720.0,
            })
            .unwrap();
            assert!(zoom >= 8);
            assert!((x1 - x0 + 1) * (y1 - y0 + 1) <= 64);
        }
    }
    #[test]
    fn corrupt_tiles_are_rejected() {
        assert!(decode_tile(b"not an image").is_err());
    }
    #[test]
    fn validates_decoded_dimensions_and_preserves_pixels() {
        let expected = RgbaImage::from_pixel(256, 256, image::Rgba([20, 80, 140, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        expected
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_tile(bytes.get_ref()).unwrap(), expected);
        let oversized = RgbaImage::new(257, 256);
        let mut bytes = std::io::Cursor::new(Vec::new());
        oversized
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(decode_tile(bytes.get_ref()).is_err());
    }
}
