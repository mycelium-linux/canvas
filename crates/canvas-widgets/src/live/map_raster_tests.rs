// Tests remain within the renderer adapter, without a GPU or provider.
use super::*;
#[test]
fn verified_tile_is_cropped_to_plot_and_missing_or_corrupt_assets_fail() {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("canvas-map-raster-test-{id}"));
    std::fs::create_dir(&root).unwrap();
    let image = image::RgbaImage::from_pixel(256, 256, image::Rgba([17, 33, 55, 255]));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let map:Map=serde_json::from_value(serde_json::json!({"extent":{"south_deg":-10,"north_deg":10,"west_deg":-10,"east_deg":10},"markers":[],"paths":[],"basemap":{"attribution":"Test fixture","tiles":[{"zoom":0,"x":0,"y":0,"width":256,"height":256,"sha256":sha,"byte_size":bytes.len()}]}})).unwrap();
    assert!(compose(&map, 500, 300, &root).is_err());
    std::fs::write(root.join(&sha), &bytes).unwrap();
    let Handle::Rgba { pixels, .. } = compose(&map, 500, 300, &root).unwrap() else {
        panic!("expected decoded retained image")
    };
    assert_eq!(
        &pixels[(150 * 500 + 250) * 4..(150 * 500 + 250) * 4 + 4],
        &[17, 33, 55, 255]
    );
    assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
    let Handle::Rgba { pixels: dark, .. } = compose_styled(&map, 500, 300, &root, true).unwrap()
    else {
        panic!("expected tactical retained image")
    };
    assert_eq!(
        &dark[(150 * 500 + 250) * 4..(150 * 500 + 250) * 4 + 4],
        &fpl_gfx::geo_map::raster::tactical_pixel([17, 33, 55, 255])
    );
    assert_eq!(&dark[..4], &[0, 0, 0, 0]);
    std::fs::write(root.join(&sha), vec![0; bytes.len()]).unwrap();
    assert!(compose(&map, 500, 300, &root).is_err());
    assert!(compose(&map, 5000, 5000, &root).is_err());
    std::fs::remove_file(root.join(sha)).unwrap();
    std::fs::remove_dir(root).unwrap();
}
