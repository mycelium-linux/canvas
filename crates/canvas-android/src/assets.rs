//! Immutable, app-private PDF assets. No URL fetching or arbitrary file paths.
use canvas_workspace::AssetRef;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;

pub fn verified_pdf(directory: &Path, asset: &AssetRef) -> Result<PathBuf, String> {
    asset.validate().map_err(|e| e.to_string())?;
    if asset.media_type != "application/pdf" || asset.byte_length > MAX_PDF_BYTES {
        return Err("Expected a PDF asset of at most 64 MiB".into());
    }
    let root = directory
        .join("assets")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let path = root.join(&asset.sha256);
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != asset.byte_length {
        return Err("PDF asset size or file type mismatch".into());
    }
    let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut length = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > asset.byte_length {
            return Err("PDF asset changed during verification".into());
        }
        hash.update(&buffer[..count]);
    }
    if length != asset.byte_length || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err("PDF asset digest mismatch".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_or_unbounded_assets() {
        for (sha256, media_type, byte_length) in [
            ("../outside".into(), "application/pdf", 1),
            ("a".repeat(64), "text/plain", 1),
            ("a".repeat(64), "application/pdf", MAX_PDF_BYTES + 1),
        ] {
            let asset = AssetRef {
                sha256,
                media_type: media_type.into(),
                byte_length,
            };
            assert!(verified_pdf(Path::new("/nonexistent"), &asset).is_err());
        }
    }
    #[test]
    fn accepts_verified_bytes_and_rejects_corruption() {
        let root = std::env::temp_dir().join(format!(
            "canvas-pdf-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let bytes = b"%PDF-1.4 test";
        let asset = AssetRef {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            media_type: "application/pdf".into(),
            byte_length: bytes.len() as u64,
        };
        let path = root.join("assets").join(&asset.sha256);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(verified_pdf(&root, &asset).unwrap(), path);
        std::fs::write(&path, b"%PDF-1.4 fail").unwrap();
        assert!(verified_pdf(&root, &asset).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root.join("assets")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
