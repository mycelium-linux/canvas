#!/usr/bin/env python3
"""Survey Prometheus metrics and emit candidate Canvas widget definitions.

Does NOT create widgets or layouts — those need human/LLM approval. This
turns "I have a Prometheus endpoint" into "here are 20 candidate widgets"
in one command, replacing the manual config/metrics.json hand-authoring.

Usage:
  canvas-prometheus-survey.py --prometheus http://127.0.0.1:9090 --output /tmp/widgets.json

The survey reads /api/v1/metadata (metric types, HELP, UNIT) and
/api/v1/label/__name__/values (metric names), applies Canvas conventions,
and emits candidate WidgetSpec JSON that a person can review, edit, and
commit to config/metrics.json.
"""
import argparse
import json
import os
import sys
import urllib.parse
import urllib.request
import urllib.error

CONTRACT = "canvas.widget.snapshot.v1"


def prom_get(base_url: str, path: str) -> dict:
    url = f"{base_url}/api/v1{path}"
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        data = json.loads(resp.read().decode())
        if data.get("status") != "success":
            raise RuntimeError(f"Prometheus {path} failed: {data}")
        return data


def infer_kind_and_config(metric_name: str, metric_type: str, help_text: str, unit: str) -> dict:
    """Infer a Canvas widget kind from Prometheus metadata."""
    name_lower = metric_name.lower()

    # Duration metrics → stat (current value is most useful)
    if name_lower.endswith("_seconds") or name_lower.endswith("_duration_seconds"):
        return {
            "kind": "stat",
            "label": help_text or metric_name,
            "unit": "s",
            "query": f"last({metric_name})",
        }

    # Byte/counter metrics → stat with trend
    if name_lower.endswith("_total") or name_lower.endswith("_bytes") or name_lower.endswith("_count"):
        return {
            "kind": "stat",
            "label": help_text or metric_name,
            "unit": unit or "bytes",
            "query": f"last({metric_name})",
        }

    # Ratio/percentage → gauge
    if name_lower.endswith("_ratio") or "percent" in name_lower or name_lower.endswith("_pct"):
        return {
            "kind": "gauge",
            "label": help_text or metric_name,
            "unit": "%",
            "query": f"last({metric_name}) * 100",
        }

    # Histograms/quantiles → heatmap (latency distributions)
    if metric_type == "histogram" or "latency" in name_lower or "duration" in name_lower:
        return {
            "kind": "heatmap",
            "label": help_text or metric_name,
            "unit": unit or "",
            "query": f"histogram_quantile(0.99, sum(rate({metric_name}[5m])) by (le))",
        }

    # Gauges → gauge or stat
    if metric_type == "gauge" or metric_type == "counter":
        return {
            "kind": "gauge",
            "label": help_text or metric_name,
            "unit": unit or "",
            "query": f"last({metric_name})",
        }

    # Fallbacks
    if "up" in name_lower or "health" in name_lower:
        return {"kind": "stat", "label": help_text or metric_name, "query": f"last({metric_name})"}

    # Default: time series for unknown metrics
    return {
        "kind": "time_series",
        "label": help_text or metric_name,
        "unit": unit or "",
        "query": f"rate({metric_name}[5m])",
    }


def survey(prom_url: str) -> list:
    """Query Prometheus metadata and return candidate widget specs."""
    # Get metric names
    names_data = prom_get(prom_url, "/label/__name__/values")
    names = names_data.get("data", {}).get("values", [])

    # Get metadata
    meta_data = prom_get(prom_url, "/metadata")
    metadata = meta_data.get("data", {})

    candidates = []
    for name in sorted(names):
        meta = metadata.get(name, [])
        if not meta:
            continue
        # Take the first metadata entry (usually the only one)
        entry = meta[0] if isinstance(meta, list) else meta
        if not isinstance(entry, dict):
            entry = {"type": "unknown", "help": "", "unit": ""}

        metric_type = entry.get("type", "unknown")
        help_text = entry.get("help", "")
        unit = entry.get("unit", "")

        config = infer_kind_and_config(name, metric_type, help_text, unit)

        # Skip internal Go runtime and Prometheus self-metrics
        if name.startswith("go_") or name.startswith("prometheus_") or name.startswith("process_"):
            continue

        candidate = {
            "id": name.replace("_", "-"),
            "kind": config["kind"],
            "label": config["label"],
            "query": config["query"],
        }
        if config.get("unit"):
            candidate["unit"] = config["unit"]
        if "warn_below" in config:
            candidate["warn_below"] = config["warn_below"]
        if "critical_below" in config:
            candidate["critical_below"] = config["critical_below"]
        if "warn_above" in config:
            candidate["warn_above"] = config["warn_above"]
        if "critical_above" in config:
            candidate["critical_above"] = config["critical_above"]

        candidates.append(candidate)

    return candidates


def main():
    parser = argparse.ArgumentParser(
        description="Survey Prometheus metrics and emit candidate Canvas widgets"
    )
    parser.add_argument("--prometheus", required=True, help="Prometheus HTTP URL")
    parser.add_argument("--output", default=None,
                        help="Output JSON file (default: stdout)")
    args = parser.parse_args()

    print(f"canvas-prometheus-survey: querying {args.prometheus}", file=sys.stderr)

    try:
        candidates = survey(args.prometheus)
    except urllib.error.URLError as exc:
        print(f"Cannot reach Prometheus at {args.prometheus}: {exc}", file=sys.stderr)
        sys.exit(1)
    except Exception as exc:
        print(f"Survey failed: {exc}", file=sys.stderr)
        sys.exit(1)

    output = {
        "schema_version": 1,
        "poll_seconds": 30,
        "sources": {},
    }

    for candidate in candidates:
        source_id = candidate.pop("id")
        output["sources"][source_id] = candidate

    text = json.dumps(output, indent=2)
    if args.output:
        with open(args.output, "w") as f:
            f.write(text)
        print(f"canvas-prometheus-survey: wrote {len(candidates)} candidates to {args.output}",
              file=sys.stderr)
    else:
        print(text)


if __name__ == "__main__":
    main()
