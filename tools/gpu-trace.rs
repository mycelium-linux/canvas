//! Read `trace-cmd report` on stdin. Report GPU job execution/queue latency
//! separately from DRM page-flip cadence; neither is an app frame timestamp.
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

#[derive(Default)]
struct Job {
    queued: Option<f64>,
    run: Option<f64>,
    key: String,
}
#[derive(Default)]
struct Samples {
    run: Vec<f64>,
    wait: Vec<f64>,
}
#[derive(Default)]
struct Trace {
    jobs: BTreeMap<String, Job>,
    samples: BTreeMap<String, Samples>,
    flips: BTreeMap<String, Vec<f64>>,
    first: Option<f64>,
    last: f64,
    lost: bool,
    lines: usize,
}
fn field<'a>(body: &'a str, prefix: &str) -> Option<&'a str> {
    body.split_whitespace()
        .find_map(|word| word.strip_prefix(prefix))
        .map(|word| word.trim_end_matches(','))
}
impl Trace {
    fn line(&mut self, line: &str) {
        if line.contains("LOST") || line.contains("tokens truncated") {
            self.lost = true;
        }
        let Some((time, body)) = line.match_indices(": ").find_map(|(index, _)| {
            let time = line[..index]
                .split_whitespace()
                .last()?
                .parse::<f64>()
                .ok()?;
            Some((time, &line[index + 2..]))
        }) else {
            return;
        };
        let Some((event, body)) = body.trim_start().split_once(':') else {
            return;
        };
        self.lines += 1;
        self.first.get_or_insert(time);
        self.last = time;
        if event == "drm_vblank_event_delivered" {
            let key = format!(
                "{}:{}",
                field(body, "file=").unwrap_or("?"),
                field(body, "crtc=").unwrap_or("?")
            );
            self.flips.entry(key).or_default().push(time);
        }
        let Some(fence) = field(body, "fence=") else {
            return;
        };
        match event {
            "drm_sched_job_queue" | "drm_sched_job_run" => {
                let key = format!(
                    "{}:{}",
                    field(body, "client_id:").unwrap_or("?"),
                    field(body, "ring=").unwrap_or("?")
                );
                let job = self.jobs.entry(fence.to_owned()).or_default();
                job.key = key;
                if event == "drm_sched_job_queue" {
                    job.queued = Some(time);
                } else {
                    job.run = Some(time);
                }
            }
            "drm_sched_job_done" => {
                if let Some(job) = self.jobs.remove(fence) {
                    if let Some(start) = job.run {
                        let samples = self.samples.entry(job.key).or_default();
                        samples.run.push((time - start) * 1000.0);
                        if let Some(queued) = job.queued {
                            samples.wait.push((start - queued) * 1000.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
fn stats(values: &[f64]) -> String {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return "null".into();
    }
    let n = values.len();
    format!(
        "{{\"n\":{n},\"p50_ms\":{:.4},\"p95_ms\":{:.4},\"p99_ms\":{:.4},\"max_ms\":{:.4},\"sum_ms\":{:.4}}}",
        values[n / 2],
        values[(n * 95 / 100).min(n - 1)],
        values[(n * 99 / 100).min(n - 1)],
        values[n - 1],
        values.iter().sum::<f64>()
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let mut trace = Trace::default();
    for line in input.lines() {
        trace.line(line);
    }
    if trace.lost {
        return Err("trace reports lost/truncated events; refuse percentiles".into());
    }
    if trace.lines == 0 {
        return Err("no trace events parsed".into());
    }
    println!(
        "{{\"duration_s\":{:.6},\"parsed_events\":{},\"gpu_jobs\":{{",
        trace.last - trace.first.unwrap(),
        trace.lines
    );
    for (i, (key, s)) in trace.samples.iter().enumerate() {
        println!(
            "{}\"{}\":{{\"execution\":{},\"queue_wait\":{}}}",
            if i == 0 { "" } else { "," },
            key,
            stats(&s.run),
            stats(&s.wait)
        );
    }
    println!("}},\"display_flips\":{{");
    for (i, (key, v)) in trace.flips.iter().enumerate() {
        let intervals: Vec<_> = v.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
        let span = v.last().unwrap() - v[0];
        let hz = if span > 0.0 {
            (v.len() - 1) as f64 / span
        } else {
            0.0
        };
        println!(
            "{}\"{}\":{{\"count\":{},\"hz\":{hz:.4},\"interval\":{}}}",
            if i == 0 { "" } else { "," },
            key,
            v.len(),
            stats(&intervals)
        );
    }
    println!("}}}}");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairs_jobs_by_fence_not_adjacent_thread() {
        let mut trace = Trace::default();
        for line in [
            "buf: app-42 [0] .... 1.000000: drm_sched_job_queue: fence=1:2, ring=v3d_render, client_id:9",
            "buf: worker-5 [0] .... 1.002000: drm_sched_job_run: fence=1:2, ring=v3d_render, client_id:9",
            "buf: irq-0 [0] .... 1.005000: drm_sched_job_done: fence=1:2 signaled",
        ] {
            trace.line(line);
        }
        let s = &trace.samples["9:v3d_render"];
        assert!((s.run[0] - 3.0).abs() < 0.0001);
        assert!((s.wait[0] - 2.0).abs() < 0.0001);
    }
    #[test]
    fn rejects_lost_and_truncated_data() {
        let mut trace = Trace::default();
        trace.line("CPU 0: LOST 50 EVENTS");
        assert!(trace.lost);
        let mut trace = Trace::default();
        trace.line("500 tokens truncated");
        assert!(trace.lost);
    }
    #[test]
    fn separates_display_outputs() {
        let mut trace = Trace::default();
        trace.line(
            "buf: irq-0 [0] .... 1.000000: drm_vblank_event_delivered: file=0xaa, crtc=3, seq=1",
        );
        trace.line(
            "buf: irq-0 [0] .... 1.016667: drm_vblank_event_delivered: file=0xaa, crtc=3, seq=2",
        );
        assert_eq!(trace.flips["0xaa:3"].len(), 2);
    }
}
