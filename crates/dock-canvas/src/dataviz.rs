//! Data visualization parsing: CSV/TSV/JSON → (x, y) point lists.
//! Extracted from main.rs for module clarity (dock-q26.14).

use std::path::Path;

pub fn parse_dataviz_file(file: &Path) -> Result<Vec<(f32, f32)>, String> {
    let content = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "csv" => parse_csv(&content),
        "json" => parse_json_data(&content),
        "tsv" => parse_csv_with_sep(&content, '\t'),
        _ => parse_csv(&content),
    }
}

fn parse_csv(content: &str) -> Result<Vec<(f32, f32)>, String> {
    parse_csv_with_sep(content, ',')
}

fn parse_csv_with_sep(content: &str, sep: char) -> Result<Vec<(f32, f32)>, String> {
    let mut data = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(sep).collect();
        if parts.len() < 2 {
            continue;
        }
        let x: f32 = parts[0]
            .trim()
            .parse()
            .map_err(|_| format!("invalid x: {}", parts[0]))?;
        let y: f32 = parts[1]
            .trim()
            .parse()
            .map_err(|_| format!("invalid y: {}", parts[1]))?;
        data.push((x, y));
    }
    if data.is_empty() {
        return Err("no data points found".into());
    }
    Ok(data)
}

pub fn parse_json_data(content: &str) -> Result<Vec<(f32, f32)>, String> {
    let value: serde_json::Value = serde_json::from_str(content).map_err(|e| e.to_string())?;
    if let Some(array) = value.as_array() {
        let mut data = Vec::new();
        for item in array {
            if item.get("x").is_some() && item.get("y").is_some() {
                let x = item["x"].as_f64().ok_or("invalid x")? as f32;
                let y = item["y"].as_f64().ok_or("invalid y")? as f32;
                data.push((x, y));
            }
        }
        Ok(data)
    } else {
        Err("JSON data must be an array of {x, y} objects".into())
    }
}
