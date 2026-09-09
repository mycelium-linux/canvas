//! Built-in named operational views.
//!
//! The durable payload is the ordinary revisioned shared-view document. This
//! catalog only maps a human-facing project/view name to that existing format.

use crate::{Error, Result, view::SharedViewDocument};

pub const NAMES: &[&str] = &["bifrost/runtime", "elmers/now"];

pub fn load(name: &str) -> Result<SharedViewDocument> {
    let source = match name {
        "bifrost/runtime" => include_str!("../../../config/views/bifrost-runtime.json"),
        "elmers/now" => include_str!("../../../config/views/elmers-now.json"),
        _ => return Err(Error(format!("Unknown operational view: {name}"))),
    };
    let document: SharedViewDocument = serde_json::from_str(source)
        .map_err(|error| Error(format!("Invalid built-in operational view {name}: {error}")))?;
    document.validate()?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_view_is_a_valid_shared_view_document() {
        for name in NAMES {
            let view = load(name).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!view.widgets.is_empty());
            assert!(!view.sources.is_empty());
        }
    }

    #[test]
    fn names_are_exact_and_unknown_views_fail_loudly() {
        assert!(load("Bifrost Runtime").is_err());
        assert!(load("bifrost/runtime").is_ok());
    }
}
