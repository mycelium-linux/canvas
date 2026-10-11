//! Finite, explicit presentation of already-provisioned immutable artifact scenes.
//! Providers own export and verification; this host owns only ordering/delivery.
use canvas_widget_host::{carousel::Cursor, presentation::call};
use serde_json::{json, Value};
use std::{collections::BTreeSet, io::Read, time::Duration};
fn documents(manifest: Value) -> Result<Vec<Value>, String> {
    if let Some(documents) = manifest.as_array() {
        return Ok(documents.clone());
    }
    if manifest["schema"] != "canvas.artifact-carousel.v1" {
        return Err("unknown artifact manifest".into());
    }
    let entries = manifest["entries"]
        .as_array()
        .ok_or("missing prepared entries")?;
    if entries.is_empty() || entries.len() > 1024 {
        return Err("invalid prepared pool size".into());
    }
    entries
        .iter()
        .map(|entry| {
            let mut document = manifest["template"].clone();
            let surfaces = document["surfaces"]
                .as_array_mut()
                .ok_or("missing template surfaces")?;
            let asset = surfaces
                .iter_mut()
                .find(|s| s["content"]["kind"] == "asset")
                .ok_or("missing asset surface")?;
            asset["content"]["asset"] = entry["asset"].clone();
            asset["title"] = entry["title"].clone();
            let caption = surfaces
                .iter_mut()
                .find(|s| s["content"]["spec"]["kind"] == "rich_card")
                .ok_or("missing caption surface")?;
            for field in ["title", "body"] {
                if !entry[field].is_string() {
                    return Err(format!("missing {field}"));
                }
                caption["content"]["spec"][field] = entry[field].clone();
            }
            Ok(document)
        })
        .collect()
}
fn validate(documents: &[Value]) -> Result<Vec<String>, String> {
    if documents.is_empty() || documents.len() > 1024 {
        return Err("expected 1..1024 scenes".into());
    }
    let project = documents[0]["project_id"]
        .as_str()
        .ok_or("missing project")?;
    let mut ids = BTreeSet::new();
    for doc in documents {
        if doc["project_id"] != project || doc["id"] != documents[0]["id"] {
            return Err("carousel may replace only one workspace".into());
        }
        let surfaces = doc["surfaces"].as_array().ok_or("missing surfaces")?;
        let assets: Vec<_> = surfaces
            .iter()
            .filter(|s| s["content"]["kind"] == "asset")
            .collect();
        if assets.len() != 1 {
            return Err("one immutable asset required per scene".into());
        }
        let asset = &assets[0]["content"]["asset"];
        let id = asset["sha256"].as_str().ok_or("missing digest")?;
        if id.len() != 64
            || !id
                .bytes()
                .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
            || asset["media_type"] != "application/pdf"
            || !asset["byte_length"]
                .as_u64()
                .is_some_and(|v| v > 0 && v <= 64 * 1024 * 1024)
        {
            return Err("invalid PDF identity or bound".into());
        }
        if !ids.insert(id.to_owned()) {
            return Err("duplicate artifact identity".into());
        }
    }
    Ok(ids.into_iter().collect())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err(
            "usage: canvas-artifact-carousel MANIFEST TVCTL ENDPOINT DWELL_SECONDS TRANSITIONS"
                .into(),
        );
    }
    let dwell: u64 = args[3].parse()?;
    let transitions: usize = args[4].parse()?;
    if !(5..=600).contains(&dwell) || !(1..=10000).contains(&transitions) {
        return Err("invalid finite dwell/transition bound".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&args[0])?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("scene manifest exceeds 4MiB".into());
    }
    let documents = documents(serde_json::from_slice(&bytes)?)?;
    let order = validate(&documents)?;
    let mut cursor = Cursor {
        current: String::new(),
    };
    for _ in 0..transitions {
        let id = cursor.successor(&order).ok_or("empty catalog")?.to_owned();
        let mut doc = documents
            .iter()
            .find(|d| {
                d["surfaces"]
                    .as_array()
                    .is_some_and(|ss| ss.iter().any(|s| s["content"]["asset"]["sha256"] == id))
            })
            .ok_or("artifact missing")?
            .clone();
        let project = doc["project_id"]
            .as_str()
            .ok_or("project missing")?
            .to_owned();
        let read = call(
            &args[1],
            &args[2],
            &json!({"schema_version":1,"request_id":"artifact-read","command":{"command":"workspace_read","name":project}}),
        )?;
        if read["ok"] != true {
            return Err("workspace not retained".into());
        }
        doc["revision"] = json!(
            read["data"]["revision"]
                .as_u64()
                .ok_or("invalid document revision")?
                + 1
        );
        let receipt = call(
            &args[1],
            &args[2],
            &json!({"schema_version":1,"request_id":"artifact-carousel","expected_revision":read["revision"],"command":{"command":"workspace_import","document":doc,"activate":false}}),
        )?;
        if receipt["ok"] != true {
            return Err("artifact import rejected".into());
        }
        println!("accepted artifact {id}");
        cursor.current = id;
        std::thread::sleep(Duration::from_secs(dwell));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn doc(digest: char) -> Value {
        json!({"id":"workspace","project_id":"project","surfaces":[{"content":{"kind":"asset","asset":{"sha256":digest.to_string().repeat(64),"media_type":"application/pdf","byte_length":100}}}]})
    }
    #[test]
    fn ordered_full_pool_reuses_shared_cursor() {
        let order = validate(&[doc('c'), doc('a'), doc('b')]).unwrap();
        let mut cursor = Cursor {
            current: String::new(),
        };
        let mut seen = Vec::new();
        for _ in 0..4 {
            cursor.current = cursor.successor(&order).unwrap().to_owned();
            seen.push(cursor.current.chars().next().unwrap());
        }
        assert_eq!(seen, vec!['a', 'b', 'c', 'a']);
    }
    #[test]
    fn rejects_unbounded_or_cross_workspace_artifacts() {
        assert!(validate(&[doc('a'), doc('a')]).is_err());
        let mut changed = doc('b');
        changed["project_id"] = json!("other");
        assert!(validate(&[doc('a'), changed]).is_err());
        assert!(validate(&[doc('Z')]).is_err());
    }
    #[test]
    fn prepared_pool_uses_one_shared_scene_template() {
        let mut template = doc('a');
        template["surfaces"]
            .as_array_mut()
            .unwrap()
            .push(json!({"content":{"kind":"widget","spec":{"kind":"rich_card"}}}));
        let manifest = json!({"schema":"canvas.artifact-carousel.v1","template":template,"entries":[{"asset":{"sha256":"b".repeat(64),"media_type":"application/pdf","byte_length":10},"title":"actual source","body":"owner evidence"}]});
        let result = documents(manifest).unwrap();
        assert_eq!(validate(&result).unwrap(), vec!["b".repeat(64)]);
        assert_eq!(
            result[0]["surfaces"][1]["content"]["spec"]["body"],
            "owner evidence"
        );
    }
}
