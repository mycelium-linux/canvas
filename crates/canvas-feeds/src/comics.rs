//! Comic-strip frame producer: the last N comics, composited into one
//! mosaic JPEG at `frames/<source_id>.jpg` for a `live_image` surface.
//!
//! The grid is composed here, in the producer, so the wall stays dumb: one
//! image, exact gutters, zero layout state in the renderer. A comic whose
//! image will not load keeps its cell empty rather than collapsing the grid
//! — an absent strip is honest, a silently-shrunken one is not.

use crate::Error;

/// One comic as its feed announced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comic {
    pub title: String,
    pub image_url: String,
}

/// The latest `count` comics from a feed's raw XML, newest first.
///
/// Comic feeds hide the image inside the item's description HTML (escaped,
/// so `field()`'s unescape exposes the tag) or in a `media:content`
/// attribute; both shapes are accepted, in that order.
pub fn latest_comics(feed_xml: &str, count: usize) -> Vec<Comic> {
    let mut out = Vec::new();
    for block in crate::item_blocks(feed_xml) {
        let Some(url) = comic_image_url(block) else {
            continue;
        };
        let title = crate::field(block, "title").unwrap_or_else(|| "(untitled)".into());
        out.push(Comic {
            title,
            image_url: url,
        });
        if out.len() >= count {
            break;
        }
    }
    out
}

fn comic_image_url(block: &str) -> Option<String> {
    let body = crate::field(block, "description")
        .or_else(|| crate::field(block, "content"))
        .or_else(|| crate::field(block, "summary"));
    if let Some(body) = body
        && let Some(url) = attr_after(&body, "<img", "src")
    {
        return Some(url);
    }
    // media:content carries the image as a raw-XML attribute (not inside the
    // description) in feeds like Atom-with-media; checked independently so an
    // entry with no description still resolves.
    attr_after(block, "media:content", "url")
}

fn attr_after(haystack: &str, tag: &str, attribute: &str) -> Option<String> {
    let at = haystack.find(tag)?;
    let needle = format!("{attribute}=\"");
    let begin = haystack[at..].find(&needle)? + at + needle.len();
    let end = haystack[begin..].find('"')? + begin;
    let value = &haystack[begin..end];
    (!value.is_empty()).then(|| value.to_owned())
}

/// Aspect-preserving fit of a `w`×`h` image inside a `box_size` square, never
/// upscaled. One scale factor for both axes is the whole point: applying it
/// separately (resize to `box×box`) is what squished wide comics.
fn fit_within(w: u32, h: u32, box_size: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let scale = (f64::from(box_size) / f64::from(w))
        .min(f64::from(box_size) / f64::from(h))
        .min(1.0);
    (
        ((f64::from(w) * scale).round().max(1.0)) as u32,
        ((f64::from(h) * scale).round().max(1.0)) as u32,
    )
}

/// Composite decoded image bytes into a fixed 2x2 mosaic of `tile`-wide
/// cells separated by `gutter` pixels (including the outer margin), on a
/// solid `background`. Missing or undecodable entries leave their cell
/// empty. Returns JPEG bytes.
pub fn mosaic(
    images: &[Option<Vec<u8>>],
    tile: u32,
    gutter: u32,
    background: [u8; 3],
) -> Result<Vec<u8>, Error> {
    use image::{ImageBuffer, Rgb, RgbImage};
    if tile == 0 || gutter == 0 {
        return Err(Error::Row {
            source: "comics".into(),
            detail: "tile and gutter must be positive".into(),
        });
    }
    let side = tile * 2 + gutter * 3;
    let mut canvas: RgbImage = ImageBuffer::from_pixel(side, side, Rgb(background));
    for (index, slot) in images.iter().take(4).enumerate() {
        let Some(bytes) = slot else { continue };
        let Ok(decoded) = image::load_from_memory(bytes) else {
            continue;
        };
        let rgb = decoded.to_rgb8();
        // Fit inside the cell preserving aspect. `imageops::resize` resamples
        // to exactly the requested dimensions, so the scale must be computed
        // here: a single factor for both axes keeps the ratio, and clamping it
        // at 1.0 means a small strip is centred rather than pixelated to fill.
        let (fw, fh) = fit_within(rgb.width(), rgb.height(), tile);
        let fitted = if (fw, fh) == (rgb.width(), rgb.height()) {
            rgb
        } else {
            image::imageops::resize(&rgb, fw, fh, image::imageops::FilterType::Lanczos3)
        };
        let col = (index % 2) as u32;
        let row = (index / 2) as u32;
        let cell_x = gutter + col * (tile + gutter);
        let cell_y = gutter + row * (tile + gutter);
        let x = cell_x + (tile - fw) / 2;
        let y = cell_y + (tile - fh) / 2;
        image::imageops::overlay(&mut canvas, &fitted, x as i64, y as i64);
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&canvas)
        .map_err(|error| Error::Row {
            source: "comics".into(),
            detail: format!("mosaic encode failed: {error}"),
        })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, RgbImage};

    fn png(color: [u8; 3], size: u32) -> Vec<u8> {
        let img: RgbImage = ImageBuffer::from_pixel(size, size, Rgb(color));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn comics_extracts_image_urls_from_escaped_descriptions() {
        let xml = "<rss><channel>\
            <item><title>Barren</title><link>https://xkcd.com/2/</link>\
            <description>&lt;img src=\"https://imgs.xkcd.com/comics/barren.png\"\
            title=\"t\" alt=\"a\" /&gt;</description></item>\
            <item><title>Sketch</title>\
            <description>&lt;img src=\"https://imgs.xkcd.com/comics/sketch.gif\" /&gt;</description></item>\
            <item><title>No image</title><description>text only</description></item>\
            </channel></rss>";
        let comics = latest_comics(xml, 4);
        assert_eq!(comics.len(), 2, "items without images are skipped");
        assert_eq!(comics[0].title, "Barren");
        assert_eq!(
            comics[0].image_url,
            "https://imgs.xkcd.com/comics/barren.png"
        );
        assert_eq!(
            comics[1].image_url,
            "https://imgs.xkcd.com/comics/sketch.gif"
        );
    }

    #[test]
    fn comics_respects_the_count_and_media_content_fallback() {
        let xml = "<feed><entry><title>a</title>\
            <media:content url=\"https://e.test/1.png\"/></entry>\
            <entry><title>b</title><media:content url=\"https://e.test/2.png\"/></entry>\
            <entry><title>c</title><media:content url=\"https://e.test/3.png\"/></entry></feed>";
        let comics = latest_comics(xml, 2);
        assert_eq!(comics.len(), 2);
        assert_eq!(comics[1].image_url, "https://e.test/2.png");
    }

    #[test]
    fn mosaic_is_a_two_by_two_grid_with_gutters() {
        let tiles = vec![
            Some(png([255, 0, 0], 100)),
            Some(png([0, 255, 0], 100)),
            Some(png([0, 0, 255], 20)),
            None,
        ];
        let bytes = mosaic(&tiles, 100, 10, [12, 12, 12]).unwrap();
        let grid = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!((grid.width(), grid.height()), (230u32, 230u32));
        // Cell centers: 2x2 of tile=100 with gutter=10 → cells at
        // x∈{60,170}, y∈{60,170}. A 20px image is centered in its cell, so
        // its own center coincides with the cell center.
        // JPEG is lossy (a pure 255 encodes back as ~254), so assert channel
        // dominance rather than exact bytes.
        let dominant = |p: &image::Rgb<u8>| -> u8 {
            let [r, g, b] = p.0;
            if r >= g && r >= b {
                0
            } else if g >= r && g >= b {
                1
            } else {
                2
            }
        };
        assert_eq!(
            grid.get_pixel(5, 5).0,
            [12, 12, 12],
            "outer margin is background"
        );
        assert_eq!(dominant(grid.get_pixel(60, 60)), 0, "top-left red");
        assert_eq!(dominant(grid.get_pixel(170, 60)), 1, "top-right green");
        assert_eq!(
            dominant(grid.get_pixel(60, 170)),
            2,
            "bottom-left blue (small, centered)"
        );
        assert_eq!(
            grid.get_pixel(170, 170).0,
            [12, 12, 12],
            "missing entry stays empty"
        );
    }

    #[test]
    fn fit_within_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_within(1600, 400, 360), (360, 90), "xkcd-shaped");
        assert_eq!(fit_within(400, 1600, 360), (90, 360), "tall");
        assert_eq!(fit_within(20, 20, 360), (20, 20), "no upscale");
        assert_eq!(fit_within(100, 100, 100), (100, 100), "exact");
    }

    #[test]
    fn mosaic_fits_a_wide_strip_centered_instead_of_squishing_it() {
        // The reported bug: wide comics were forced into the square tile.
        let img: RgbImage = ImageBuffer::from_pixel(200, 25, Rgb([255, 0, 0]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let bytes = mosaic(&[Some(png), None, None, None], 100, 10, [12, 12, 12]).unwrap();
        let grid = image::load_from_memory(&bytes).unwrap().to_rgb8();
        // 200x25 fit to 100x100 is 100x13, centred in the top-left cell
        // (cell spans 10..110): red across the full width at mid-cell height,
        // background at the cell's top where a squished square would have bled.
        let [r, g, b] = grid.get_pixel(60, 60).0;
        assert!(
            r > 200 && g < 60 && b < 60,
            "strip mid-body {:?}",
            [r, g, b]
        );
        assert_eq!(
            grid.get_pixel(60, 15).0,
            [12, 12, 12],
            "cell top stays background"
        );
    }

    #[test]
    fn mosaic_rejects_degenerate_geometry() {
        assert!(mosaic(&[], 0, 10, [0, 0, 0]).is_err());
    }
}
