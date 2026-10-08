//! Immutable, app-private PDF assets. No URL fetching or arbitrary file paths.
use canvas_workspace::AssetRef;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PdfViewportPage {
    pub bbox: [f64; 4],
    pub source_sha256: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PdfViewports {
    pub schema_version: u32,
    pub pdf_sha256: String,
    pub pages: Vec<PdfViewportPage>,
}

impl PdfViewports {
    pub fn validate(&self, asset: &AssetRef) -> Result<(), String> {
        if self.schema_version != 1
            || self.pdf_sha256 != asset.sha256
            || self.pages.is_empty()
            || self.pages.len() > 512
        {
            return Err("PDF drawing bounds identity or page count mismatch".into());
        }
        for page in &self.pages {
            let [x, y, w, h] = page.bbox;
            if !page.bbox.iter().all(|v| v.is_finite())
                || x < 0.0
                || y < 0.0
                || w <= 0.0
                || h <= 0.0
                || x + w > 1.0
                || y + h > 1.0
                || page.source_sha256.len() != 64
                || !page
                    .source_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("Invalid PDF drawing bounds or source digest".into());
            }
        }
        Ok(())
    }
}

/// Optional local renderer metadata, bound to the immutable PDF digest.
pub fn verified_pdf_viewports(
    directory: &Path,
    asset: &AssetRef,
) -> Result<Option<PdfViewports>, String> {
    asset.validate().map_err(|e| e.to_string())?;
    let path = directory
        .join("assets")
        .join(format!("{}.viewports.json", asset.sha256));
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("PDF drawing bounds must be a regular file of at most 64 KiB".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("PDF drawing bounds exceeded size limit".into());
    }
    let value: PdfViewports = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    value.validate(asset)?;
    Ok(Some(value))
}

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
    fn drawing_bounds_are_source_bound_and_bounded() {
        let asset = AssetRef {
            sha256: "a".repeat(64),
            media_type: "application/pdf".into(),
            byte_length: 1,
        };
        let mut bounds = PdfViewports {
            schema_version: 1,
            pdf_sha256: asset.sha256.clone(),
            pages: vec![PdfViewportPage {
                bbox: [0.2, 0.1, 0.6, 0.4],
                source_sha256: "b".repeat(64),
            }],
        };
        assert!(bounds.validate(&asset).is_ok());
        for bad in [
            [0., 0., f64::NAN, 0.1],
            [0., 0., 0., 1.],
            [0.8, 0., 0.3, 1.],
            [-0.1, 0., 0.1, 1.],
        ] {
            bounds.pages[0].bbox = bad;
            assert!(bounds.validate(&asset).is_err());
        }
        bounds.pages[0].bbox = [0.2, 0.1, 0.6, 0.4];
        bounds.pdf_sha256 = "c".repeat(64);
        assert!(bounds.validate(&asset).is_err());
        bounds.pdf_sha256 = asset.sha256.clone();
        bounds.pages.clear();
        assert!(bounds.validate(&asset).is_err());
    }
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
        assert!(verified_pdf_viewports(&root, &asset).unwrap().is_none());
        let sidecar = root
            .join("assets")
            .join(format!("{}.viewports.json", asset.sha256));
        let bounds = PdfViewports {
            schema_version: 1,
            pdf_sha256: asset.sha256.clone(),
            pages: vec![PdfViewportPage {
                bbox: [0.2, 0.1, 0.6, 0.4],
                source_sha256: "b".repeat(64),
            }],
        };
        std::fs::write(&sidecar, serde_json::to_vec(&bounds).unwrap()).unwrap();
        assert_eq!(
            verified_pdf_viewports(&root, &asset)
                .unwrap()
                .unwrap()
                .pages
                .len(),
            1
        );
        std::fs::write(&sidecar, b"{}").unwrap();
        assert!(verified_pdf_viewports(&root, &asset).is_err());
        std::fs::write(&sidecar, vec![b' '; 64 * 1024 + 1]).unwrap();
        assert!(verified_pdf_viewports(&root, &asset).is_err());
        std::fs::remove_file(sidecar).unwrap();
        std::fs::write(&path, b"%PDF-1.4 fail").unwrap();
        assert!(verified_pdf(&root, &asset).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root.join("assets")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
