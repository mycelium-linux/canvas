//! Classifies a Prometheus metadata survey into candidate widget definitions.
//!
//! This process reads facts and writes files. It never scrapes: collecting the
//! survey is the caller's job, because the classification is the part worth
//! testing and a transform that also opens sockets is a transform nobody can
//! test. Feed it the metadata and cardinality of the metrics you already have,
//! from `/api/v1/metadata` and the series index, and it proposes widgets.
//!
//! Nothing is written unless `--out` says where. Candidates are proposals: a
//! reviewer reads the rationale, corrects the guesses that are wrong, and copies
//! the two files in.
use canvas_survey::{MetricFact, Survey, SurveyOptions, survey};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const USAGE: &str = "\
canvas-survey — Prometheus metadata to candidate Canvas widgets

    canvas-survey [--facts <path|->] [--out <dir>] [--json]
                  [--window <seconds>] [--step <seconds>]
                  [--rate-window <duration>] [--poll <seconds>]

    --facts        Survey JSON: an array of {name, type, series, labels, help,
                   unit}. Defaults to stdin.
    --out          Write config/metrics.json and widgets/<id>.json under this
                   directory. Without it nothing is written.
    --json         Print the whole survey, rationale included, instead of the
                   report.
    --window       Generate for a trend wall over this many seconds. Without it,
                   for an instant status board — the two disagree about what a
                   multi-series gauge should be.
    --step         Range-query step. Default 30.
    --rate-window  Range every rate() is taken over. Default 5m.
    --poll         Poll interval written into the mapping. Default 30.
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|candidate| candidate == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn number(args: &[String], flag: &str) -> Result<Option<u64>, String> {
    match arg(args, flag) {
        None => Ok(None),
        Some(raw) => raw
            .parse()
            .map(Some)
            .map_err(|error| format!("{flag} takes a number: {raw} ({error})")),
    }
}

fn read_facts(source: &str) -> Result<String, String> {
    if source == "-" {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        return Ok(buffer);
    }
    std::fs::read_to_string(source).map_err(|error| format!("cannot read {source}: {error}"))
}

/// Writes the two shapes a reviewer copies from, side by side under one root.
fn write_out(survey: &Survey, root: &Path) -> Result<Vec<PathBuf>, String> {
    let widgets = root.join("widgets");
    std::fs::create_dir_all(&widgets)
        .map_err(|error| format!("cannot create {}: {error}", widgets.display()))?;

    let mut written = Vec::new();
    let mut write = |path: PathBuf, value: &serde_json::Value| -> Result<(), String> {
        let body = serde_json::to_string_pretty(value)
            .map_err(|error| format!("cannot serialize {}: {error}", path.display()))?;
        std::fs::write(&path, body + "\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        written.push(path);
        Ok(())
    };

    write(root.join("metrics.json"), &survey.mapping_json())?;
    for (name, widget) in survey.widget_files() {
        write(widgets.join(name), &widget)?;
    }
    Ok(written)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return Ok(());
    }

    let mut options = SurveyOptions {
        window_seconds: number(&args, "--window")?,
        ..SurveyOptions::default()
    };
    if let Some(step) = number(&args, "--step")? {
        options.step_seconds = step;
    }
    if let Some(poll) = number(&args, "--poll")? {
        options.poll_seconds = poll;
    }
    if let Some(rate_window) = arg(&args, "--rate-window") {
        options.rate_window = rate_window;
    }

    let source = arg(&args, "--facts").unwrap_or_else(|| "-".into());
    let raw = read_facts(&source)?;
    let facts: Vec<MetricFact> = serde_json::from_str(&raw)
        .map_err(|error| format!("cannot parse survey facts from {source}: {error}"))?;
    let survey = survey(&facts, &options)?;

    match arg(&args, "--out") {
        Some(root) => {
            let written = write_out(&survey, Path::new(&root))?;
            eprint!("{}", survey.report());
            for path in written {
                println!("{}", path.display());
            }
        }
        None if args.iter().any(|arg| arg == "--json") => {
            let json = serde_json::to_string_pretty(&survey)
                .map_err(|error| format!("cannot serialize survey: {error}"))?;
            println!("{json}");
        }
        None => print!("{}", survey.report()),
    }
    Ok(())
}
