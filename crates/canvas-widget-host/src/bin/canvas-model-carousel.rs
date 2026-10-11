//! Lazy, bounded client of the owner STEP→GLB converter and Canvas model host.
//! Immutable owner STEP/receipts are read-only; only our staging/GLB cache evicts.
use serde::Deserialize;
use serde_json::json;
use std::{
    env, fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
struct Asset {
    step: String,
    receipt: String,
    sha256: String,
    geometry: f64,
    training_admitted: bool,
    #[serde(default)]
    feature_counts: Option<std::collections::BTreeMap<String, u32>>,
    #[serde(default)]
    feature_error: Option<String>,
    #[serde(default)]
    rendered_at_ms: Option<i64>,
}
#[derive(Deserialize)]
struct Catalog {
    schema: String,
    source_root: String,
    observed_at: String,
    assets: Vec<Asset>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    catalog: PathBuf,
    local_staging: PathBuf,
    studio: String,
    spark: String,
    jump: String,
    studio_assets: String,
    owner_cli: String,
    scenario: PathBuf,
    orbit_count: u32,
    degrees_per_second: f64,
    duration_seconds: u64,
}
#[derive(Deserialize)]
struct RenderState {
    current_asset: Option<String>,
    next_asset: Option<String>,
    next_ready: bool,
    completed_orbits: u32,
    observed_at_ms: i64,
}
fn read_render_state(config: &Config) -> Result<RenderState, String> {
    let bytes = ssh(
        &config.studio,
        format!("cat {}/render-state.json", config.studio_assets),
        5,
    )?;
    let state: RenderState = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if chrono::Utc::now().timestamp_millis() - state.observed_at_ms > 10_000 {
        return Err("renderer acknowledgement expired".into());
    }
    Ok(state)
}
fn command(program: &str, args: &[String], seconds: u64) -> Result<Vec<u8>, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("missing command output")?;
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let bytes = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => result.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("command deadline exceeded".into());
        }
    };
    if bytes.len() > 64 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("command output exceeds bound".into());
    }
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(bytes)
            } else {
                Err(format!("{program} failed: {status}"))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("command exit deadline exceeded".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn ssh(host: &str, request: String, seconds: u64) -> Result<Vec<u8>, String> {
    command(
        "/usr/bin/ssh",
        &[
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            "-o".into(),
            "ServerAliveInterval=2".into(),
            "-o".into(),
            "ServerAliveCountMax=2".into(),
            host.into(),
            request,
        ],
        seconds,
    )
}
fn safe_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
}
fn refresh_catalog(config: &Config, previous: &Catalog) -> Result<Catalog, String> {
    let helper = env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("canvas-model-catalog");
    let owner_command=format!("find {} -type f -name verified.json -exec jq -c 'select(.rebuilt_readback_valid == true) | {{receipt:input_filename,step:(input_filename|sub(\"verified.json$\";\"rebuilt.step\")),sha256:.rebuilt_step_sha256,geometry:.geometry.symmetric_difference_mm3,training_admitted:.training_admitted}}' {{}} +",previous.source_root);
    command(
        helper.to_str().ok_or("non UTF-8 catalog helper")?,
        &[
            config.catalog.to_string_lossy().into_owned(),
            previous.source_root.clone(),
            "/usr/bin/ssh".into(),
            "-J".into(),
            config.jump.clone(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            "-o".into(),
            "ServerAliveInterval=2".into(),
            "-o".into(),
            "ServerAliveCountMax=2".into(),
            config.spark.clone(),
            owner_command,
        ],
        30,
    )?;
    let fresh: Catalog =
        serde_json::from_slice(&fs::read(&config.catalog).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if fresh.schema != "canvas.model-catalog.v1"
        || fresh.source_root != previous.source_root
        || fresh.assets.is_empty()
        || fresh.assets.len() > 100_000
    {
        return Err("refreshed catalog contract mismatch".into());
    }
    Ok(fresh)
}
struct StagingCleanup {
    config: Config,
    local: PathBuf,
    remote: Option<String>,
}
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        if let Some(remote) = &self.remote {
            if let Err(error) = ssh(&self.config.studio, format!("rm -rf {remote}"), 10) {
                eprintln!("derived staging cleanup failed: {error}");
            }
        }
        if self.local.is_file() {
            if let Err(error) = fs::remove_file(&self.local) {
                eprintln!("local STEP copy cleanup failed: {error}");
            }
        }
    }
}
fn prepare(config: &Config, asset: &mut Asset) -> Result<String, String> {
    let sha = &asset.sha256;
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || !safe_path(&asset.step) {
        return Err("unsafe owner artifact identity".into());
    }
    let recognition = asset
        .step
        .strip_suffix("rebuilt.step")
        .ok_or("invalid STEP filename")?
        .to_owned()
        + "recognition.json";
    let features = command(
        "/usr/bin/ssh",
        &[
            "-J".into(),
            config.jump.clone(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            config.spark.clone(),
            format!("jq -c '.recognized_feature_counts' {recognition}"),
        ],
        15,
    )
    .and_then(|bytes| {
        serde_json::from_slice::<std::collections::BTreeMap<String, u32>>(&bytes)
            .map_err(|e| e.to_string())
    })
    .and_then(|counts| {
        if counts.len() <= 64
            && counts.iter().all(|(key, count)| {
                key.len() <= 80
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && *count <= 100_000
            })
        {
            Ok(counts)
        } else {
            Err("invalid owner feature counts".into())
        }
    });
    match features {
        Ok(counts) => asset.feature_counts = Some(counts),
        Err(detail) => {
            eprintln!(
                "owner feature metadata unavailable {}: {detail}",
                asset.sha256
            );
            asset.feature_error = Some("Owner recognition metadata unavailable".into());
        }
    }
    fs::create_dir_all(&config.local_staging).map_err(|e| e.to_string())?;
    let step = config.local_staging.join(format!("{sha}.step"));
    let mut cleanup = StagingCleanup {
        config: config.clone(),
        local: step.clone(),
        remote: None,
    };
    command(
        "/usr/bin/scp",
        &[
            "-o".into(),
            format!("ProxyJump={}", config.jump),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            format!("{}:{}", config.spark, asset.step),
            step.to_string_lossy().into_owned(),
        ],
        30,
    )?;
    if fs::metadata(&step).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
        return Err("STEP staging exceeds8MiB".into());
    }
    let hash = command(
        "/usr/bin/shasum",
        &[
            "-a".into(),
            "256".into(),
            step.to_string_lossy().into_owned(),
        ],
        5,
    )?;
    if std::str::from_utf8(&hash)
        .map_err(|e| e.to_string())?
        .split_whitespace()
        .next()
        != Some(sha)
    {
        return Err("owner STEP digest mismatch".into());
    }
    let stage = format!("{}/staging-{sha}", config.studio_assets);
    ssh(&config.studio, format!("mkdir -p {stage}"), 10)?;
    cleanup.remote = Some(stage.clone());
    command(
        "/usr/bin/scp",
        &[
            "-o".into(),
            "BatchMode=yes".into(),
            config.scenario.to_string_lossy().into_owned(),
            format!("{}:{stage}/scenario.lua", config.studio),
        ],
        20,
    )?;
    command(
        "/usr/bin/scp",
        &[
            "-o".into(),
            "BatchMode=yes".into(),
            step.to_string_lossy().into_owned(),
            format!("{}:{stage}/input.step", config.studio),
        ],
        20,
    )?;
    let asset_name = format!("model-{sha}.glb");
    ssh(
        &config.studio,
        format!(
            "{} import-part {stage} recovered {stage}/input.step",
            config.owner_cli
        ),
        60,
    )?;
    let bytes = ssh(
        &config.studio,
        format!("stat -f%z {stage}/out/recovered/parts/0.glb"),
        10,
    )?;
    let size: u64 = std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())?
        .trim()
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    if size == 0 || size > 32 * 1024 * 1024 {
        return Err("derived preview exceeds per-model32MiB budget".into());
    }
    ssh(
        &config.studio,
        format!(
            "cp {stage}/out/recovered/parts/0.glb {}/{asset_name}",
            config.studio_assets
        ),
        10,
    )?;
    // This exact content-addressed staging directory was created by this client.
    // It contains copies, not the immutable STEP/receipts on the owner archive.
    drop(cleanup);
    Ok(asset_name)
}
fn scene(config: &Config, asset: &str, next: Option<&str>) -> Result<(), String> {
    let next_field = next
        .map(|name| format!(",next_asset=\"{name}\""))
        .unwrap_or_default();
    let text=format!("return {{scene_id=\"verk-recovered-showcase\",asset=\"{asset}\"{next_field},model_rotation_degrees={{-90,0,0}},orbit={{axis={{0,1,0}},degrees_per_second={}}},camera=\"iso\",camera_distance=1.75,crossfade_ms=500,material={{color={{178,196,212}},glow=0,wireframe=false,opacity=1}},transition_ms=500,fly_in=false}}",config.degrees_per_second);
    let local = config.local_staging.join("scene.lua");
    fs::write(&local, text).map_err(|e| e.to_string())?;
    command(
        "/usr/bin/scp",
        &[
            "-o".into(),
            "BatchMode=yes".into(),
            local.to_string_lossy().into_owned(),
            format!(
                "{}:{}/scene.pending.lua",
                config.studio, config.studio_assets
            ),
        ],
        20,
    )?;
    ssh(
        &config.studio,
        format!(
            "mv {}/scene.pending.lua {}/scene.lua",
            config.studio_assets, config.studio_assets
        ),
        10,
    )?;
    Ok(())
}
fn emit(catalog: &Catalog, current: Option<&Asset>, pending: Option<&Asset>, failures: usize) {
    let value = json!({"schema":"canvas.model-showcase.v1","observed_at_unix_ms":current.and_then(|asset|asset.rendered_at_ms).unwrap_or_else(||chrono::Utc::now().timestamp_millis()),"catalog_observed_at":catalog.observed_at,"catalog_count":catalog.assets.len(),"current":current.map(|a|json!({"step":a.step,"receipt":a.receipt,"sha256":a.sha256,"geometry":a.geometry,"training_admitted":a.training_admitted,"feature_counts":a.feature_counts,"feature_error":a.feature_error})),"pending":pending.map(|a|json!({"step":a.step,"sha256":a.sha256})),"failed_previews":failures,"identity_state":"renderer_acknowledged"});
    println!("{value}");
    let _ = std::io::stdout().flush();
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("operator config required")?;
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    for path in [&config.studio_assets, &config.owner_cli] {
        if !safe_path(path) {
            return Err("unsafe operator path".into());
        }
    }
    if !(1..=10).contains(&config.orbit_count)
        || !(1.0..=90.0).contains(&config.degrees_per_second)
        || !(1..=86400).contains(&config.duration_seconds)
    {
        return Err("invalid bounded carousel settings".into());
    }
    let mut catalog: Catalog = serde_json::from_slice(&fs::read(&config.catalog)?)?;
    if catalog.schema != "canvas.model-catalog.v1"
        || catalog.assets.is_empty()
        || catalog.assets.len() > 100_000
        || !safe_path(&catalog.source_root)
    {
        return Err("invalid owner catalog".into());
    }
    catalog.assets.sort_by(|a, b| a.step.cmp(&b.step));
    let started = Instant::now();
    let mut cursor = 0usize;
    let mut current: Option<Asset> = None;
    let mut current_name: Option<String> = None;
    let mut failures = 0usize;
    // Resume from the renderer's acknowledged identity, not an independently
    // maintained counter that might have advanced while the producer was down.
    if let Ok(state) = read_render_state(&config) {
        if let Some(name) = state.current_asset {
            if let Some(index) = catalog
                .assets
                .iter()
                .position(|asset| name == format!("model-{}.glb", asset.sha256))
            {
                let mut asset = catalog.assets[index].clone();
                asset.rendered_at_ms = Some(state.observed_at_ms);
                current = Some(asset);
                current_name = Some(name);
                cursor = (index + 1) % catalog.assets.len();
            }
        }
    }
    let mut preparation: Option<mpsc::Receiver<(Asset, Result<String, String>)>> = None;
    let mut ready: Option<(Asset, String)> = None;
    let mut requested: Option<(Asset, String)> = None;
    let mut last_emit = Instant::now();
    while started.elapsed() < Duration::from_secs(config.duration_seconds) {
        if preparation.is_none() && ready.is_none() && requested.is_none() {
            if cursor == 0 && current.is_some() {
                // Finish one stable generation before accepting a growing owner
                // inventory, so newly arriving parts cannot starve older parts.
                match refresh_catalog(&config, &catalog) {
                    Ok(mut fresh) => {
                        fresh.assets.sort_by(|a, b| a.step.cmp(&b.step));
                        catalog = fresh;
                    }
                    Err(error) => eprintln!(
                        "owner inventory refresh failed; retaining previous generation: {error}"
                    ),
                }
            }
            let mut asset = catalog.assets[cursor].clone();
            cursor = (cursor + 1) % catalog.assets.len();
            let cloned = config.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            emit(&catalog, current.as_ref(), Some(&asset), failures);
            thread::spawn(move || {
                let result = prepare(&cloned, &mut asset);
                let _ = tx.send((asset, result));
            });
            preparation = Some(rx);
        }
        if let Some(receiver) = &preparation {
            if let Ok((asset, result)) = receiver.try_recv() {
                preparation = None;
                match result {
                    Ok(name) => {
                        if let Some(current_name) = &current_name {
                            scene(&config, current_name, Some(&name))?;
                            ready = Some((asset, name));
                        } else {
                            scene(&config, &name, None)?;
                            requested = Some((asset, name));
                        }
                    }
                    Err(detail) => {
                        failures += 1;
                        eprintln!("preload failure {}: {detail}", asset.sha256);
                    }
                }
                emit(&catalog, current.as_ref(), None, failures);
            }
        }
        if let Ok(state) = read_render_state(&config) {
            if state.current_asset == current_name {
                if let Some(asset) = &mut current {
                    asset.rendered_at_ms = Some(state.observed_at_ms);
                }
            }
            if requested
                .as_ref()
                .is_some_and(|(_, name)| state.current_asset.as_ref() == Some(name))
            {
                let (mut asset, name) = requested.take().expect("checked requested model");
                asset.rendered_at_ms = Some(state.observed_at_ms);
                // A successor has been baked, uploaded, crossfaded, read back and emitted.
                // Only now is the former resident derived GLB eligible for eviction.
                if let Some(old) = current_name.replace(name.clone()) {
                    if old != name {
                        ssh(
                            &config.studio,
                            format!("rm -f {}/{old}", config.studio_assets),
                            10,
                        )?;
                    }
                }
                current = Some(asset);
                emit(&catalog, current.as_ref(), None, failures);
            }
            if requested.is_none()
                && ready.as_ref().is_some_and(|(_, name)| {
                    state.next_ready
                        && state.next_asset.as_ref() == Some(name)
                        && state.completed_orbits >= config.orbit_count
                })
            {
                let (asset, name) = ready.take().expect("checked ready model");
                scene(&config, &name, None)?;
                requested = Some((asset, name));
            }
        }
        if last_emit.elapsed() >= Duration::from_secs(10) {
            emit(&catalog, current.as_ref(), None, failures);
            last_emit = Instant::now();
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
