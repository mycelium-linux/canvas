use canvas_marbles::{Config, document, fetch, install_request, presentation};
use std::io::Write;

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(args.len() == 3 && ["--document", "--once", "--watch"].contains(&args[2].as_str())
        || args.len() == 5 && args[2] == "--install-request")
    {
        return Err("Usage: canvas-marbles CONFIG.json --document|--once|--watch|--install-request CHECKPOINT OWNER".into());
    }
    let config: Config =
        serde_json::from_slice(&std::fs::read(&args[1]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    config.validate()?;
    let mut output = std::io::stdout().lock();
    if args[2] == "--install-request" {
        let bytes = std::fs::read(&args[3]).map_err(|_| "Cannot read owner checkpoint")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Checkpoint exceeds receiver limit".into());
        }
        let checkpoint = serde_json::from_slice(&bytes).map_err(|_| "Invalid owner checkpoint")?;
        serde_json::to_writer(
            &mut output,
            &install_request(&config, &checkpoint, &args[4])?,
        )
        .map_err(|e| e.to_string())?;
        writeln!(output).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if args[2] == "--document" {
        serde_json::to_writer(&mut output, &document(&config.workspace))
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    loop {
        let result = fetch(&config);
        if let Err(error) = &result {
            eprintln!("canvas-marbles: {error}");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock precedes epoch")?
            .as_secs();
        let now = i64::try_from(now).map_err(|_| "System clock exceeds timestamp range")?;
        let frames = presentation(result.as_deref().map_err(String::as_str), &config, now);
        let healthy = frames[0]["data"]["state"] == "ready";
        for frame in frames {
            serde_json::to_writer(&mut output, &frame).map_err(|e| e.to_string())?;
            writeln!(output).map_err(|e| e.to_string())?;
        }
        output.flush().map_err(|e| e.to_string())?;
        if !healthy {
            eprintln!(
                "canvas-marbles: owner projection stale; check owner connection or cohort capacity"
            );
        }
        if args[2] == "--once" {
            return if healthy {
                Ok(())
            } else {
                Err("Owner projection is stale".into())
            };
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("canvas-marbles: {error}");
        std::process::exit(1);
    }
}
