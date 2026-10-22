#!/usr/bin/env python3
"""
Prometheus survey tool for Canvas.

Surveys a Prometheus job via `fpl-desktop-bridge metric-read` and emits
CANDIDATE widget definitions — not a layout, not a deployed dashboard.
A person or LLM finishes it. This is what turns Canvas from a sandbox into
a dashboarding tool: a new service becomes a dashboard in minutes rather
than a hand-written mapping.

Usage:
  python3 scripts/prometheus-survey.py --job bifrost --output candidates.json

The output is a JSON file with candidates, each annotated with the rule
that fired and the evidence (type, cardinality, unit, name) so a reviewer
can correct a wrong guess rather than re-deriving it.

Widget kind selection rules (from Bead notes):
  - counter, any cardinality     -> time_series of rate(...[5m])
  - gauge, 1 series              -> stat
  - gauge, 2-12 series           -> ranked_bar
  - gauge, >12 series            -> top-N ranked_bar (tail disclosed as event_log)
  - histogram (_bucket)          -> histogram or heatmap
  - gauge in 0..1, or _ratio     -> gauge (scaled x100)
  - _info                        -> NOT a widget (label enrichment)
  - up                           -> status_list
  - X_used_bytes + X_bytes       -> ONE utilisation gauge (family detection)
  - X_total + X_errors_total     -> error rate time_series
  - _sum + _count                -> average stat
"""
import argparse
import json
import os
import subprocess
import sys
from collections import defaultdict
from typing import Optional

# Name suffixes that signal a metric's shape.
NAME_SUFFIXES = {
    "_total": "counter",
    "_info": "info",
    "_ratio": "ratio",
    "_seconds": "duration",
    "_bytes": "bytes",
    "_bucket": "histogram",
    "_sum": "summary_sum",
    "_count": "summary_count",
    "_used_bytes": "used_bytes",
}


def read_bridge(bridge: str, action: str, query: str) -> Optional[dict]:
    """Call fpl-desktop-bridge metric-read and return parsed JSON."""
    cmd = [bridge, "metric-read", action, query]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        if not result.returncode == 0:
            return None
        return json.loads(result.stdout)
    except (subprocess.TimeoutExpired, json.JSONDecodeError, FileNotFoundError):
        return None


def get_metadata(bridge: str, metric: Optional[str] = None) -> dict:
    """Fetch metric metadata from Prometheus via the bridge."""
    query = metric or ""
    result = read_bridge(bridge, "metadata", query)
    if result is None:
        return {}
    # Result shape: {"data": {"metric_name": [{"type": "gauge", "help": "...", "unit": "bytes"}]}}
    data = result.get("data", {})
    metadata = {}
    for name, entries in data.items():
        if entries:
            metadata[name] = entries[0]  # Take first entry per metric
    return metadata


def get_metric_names(bridge: str) -> list:
    """Fetch all metric names from Prometheus via the bridge."""
    result = read_bridge(bridge, "metric_names", "")
    if result is None:
        return []
    # Result shape: {"data": ["metric1", "metric2", ...]}
    return result.get("data", [])


def get_series(bridge: str, job: str) -> list:
    """Fetch all series for a Prometheus job."""
    result = read_bridge(bridge, "series", f"job={job}")
    if result is None:
        return []
    # Result shape: {"data": [{"__name__": "metric", "job": "x", "instance": "y"}, ...]}
    return result.get("data", [])


def name_type(name: str) -> str:
    """Classify a metric name by its suffix."""
    for suffix, typ in NAME_SUFFIXES.items():
        if name.endswith(suffix):
            return typ
    return "unknown"


def cardinality(series_list: list, metric_name: str) -> int:
    """Count distinct label combinations for a metric."""
    seen = set()
    for series in series_list:
        if series.get("__name__") == metric_name:
            # Key on all labels except __name__
            key = tuple(sorted(
                (k, v) for k, v in series.items() if k != "__name__"
            ))
            seen.add(key)
    return len(seen)


def classify(metric_name: str, metadata: dict, card: int) -> tuple:
    """
    Classify a metric into (widget_kind, rationale).

    Returns (kind, rationale) where rationale explains why this kind
    was chosen, citing type, cardinality, unit, and name evidence.
    """
    meta = metadata.get(metric_name, {})
    mtype = meta.get("type", "unknown")
    unit = meta.get("unit", "")
    help_text = meta.get("help", "")
    nt = name_type(metric_name)

    # Special: up is always a status list
    if metric_name == "up":
        return ("status_list", "up: job availability is a status, not a number")

    # Info metrics are not widgets — they enrich other metrics
    if nt == "info" or metric_name.endswith("_info"):
        return ("skip", f"{metric_name}: _info metric, label enrichment only")

    # Family detection: X_used_bytes + X_bytes → utilisation gauge
    if nt == "used_bytes":
        base = metric_name[:-11]  # strip "_used_bytes"
        partner = f"{base}_bytes"
        if partner in metadata:
            return ("stat", f"family: {base}_used_bytes + {base}_bytes → utilisation gauge ({base}_utilization)")

    # Histogram buckets
    if nt == "histogram" or metric_name.endswith("_bucket"):
        if card <= 12:
            return ("histogram", f"histogram: {mtype} type, {card} series")
        return ("heatmap", f"histogram: {mtype} type, {card} series → heatmap")

    # Summary: _sum + _count → average
    if nt in ("summary_sum", "summary_count"):
        return ("stat", f"summary: {nt} → average stat from _sum/_count pair")

    # Counter → time_series of rate
    if mtype == "counter" or nt == "counter":
        return ("time_series", f"counter: rate(...[5m]) for {metric_name}")

    # Gauge
    if mtype == "gauge" or nt == "gauge":
        # 0..1 or _ratio → gauge
        if nt == "ratio" or (unit and unit in ("ratio", "percent")):
            return ("gauge", f"gauge: ratio/percent unit, scaled 0-100")
        # Family: X_total + X_errors_total → error rate
        if metric_name.endswith("_total"):
            base = metric_name[:-6]
            errors = f"{base}_errors_total"
            if errors in metadata:
                return ("time_series", f"family: {base}_total + {base}_errors_total → error rate")
        if card == 1:
            return ("stat", f"gauge: single series {card}")
        if card <= 12:
            return ("ranked_bar", f"gauge: {card} series → ranked bar")
        return ("ranked_bar", f"gauge: {card} series → top-N ranked bar (tail as event_log)")

    # Untyped → report, don't guess
    return ("unclassified", f"unclassified: {mtype} type, {card} series, unit={unit}")


def family_detect(metric_name: str, metadata: dict) -> Optional[str]:
    """Detect metric families that collapse into one widget."""
    nt = name_type(metric_name)
    if nt == "used_bytes":
        base = metric_name[:-11]
        return f"utilisation:{base}"
    if nt == "summary_sum":
        base = metric_name[:-4]  # strip "_sum"
        return f"average:{base}"
    if metric_name.endswith("_total") and f"{metric_name}_errors_total" in metadata:
        base = metric_name[:-6]
        return f"error_rate:{base}"
    return None


def survey(bridge: str, job: str) -> list:
    """Survey a Prometheus job and return candidate widget definitions."""
    # Step 1: Get all metric names
    names = get_metric_names(bridge)
    if not names:
        print(f"No metrics found for job '{job}'", file=sys.stderr)
        return []

    # Step 2: Get metadata for each metric (batch)
    metadata = get_metadata(bridge, None) or {}

    # Step 3: Get series to determine cardinality
    series = get_series(bridge, job)

    # Step 4: Classify each metric
    candidates = []
    seen_families = set()
    for name in names:
        card = cardinality(series, name)
        kind, rationale = classify(name, metadata, card)

        if kind == "skip":
            continue

        family = family_detect(name, metadata)
        if family:
            if family in seen_families:
                # Skip individual metrics of a family we've already emitted
                continue
            seen_families.add(family)

        meta = metadata.get(name, {})
        candidate = {
            "metric": name,
            "widget_kind": kind,
            "source_id": f"{job}/{name}",
            "provenance": "survey",
            "evidence": {
                "type": meta.get("type", ""),
                "unit": meta.get("unit", ""),
                "help": meta.get("help", "")[:200],
                "cardinality": card,
                "name_type": name_type(name),
            },
            "rule": rationale,
            "spec": {
                "schema_version": 2,
                "id": f"{job}-{name}",
                "kind": kind,
                "label": meta.get("help", name) or name,
                "data": {
                    "state": "loading",
                    "source": f"prometheus:{job}/{name}",
                    "provenance": "demo",
                },
            },
        }
        candidates.append(candidate)

    # Sort by family then name for readability
    candidates.sort(key=lambda c: (c["rule"], c["metric"]))
    return candidates


def main():
    parser = argparse.ArgumentParser(description="Survey Prometheus for Canvas widget candidates")
    parser.add_argument("--job", required=True,
                        help="Prometheus job name to survey")
    parser.add_argument("--bridge", default=os.environ.get("FPL_DESKTOP_BRIDGE_BIN", "fpl-desktop-bridge"),
                        help="Path to fpl-desktop-bridge binary")
    parser.add_argument("--output", "-o", default=None,
                        help="Output file (default: stdout)")
    args = parser.parse_args()

    print(f"Surveying job '{args.job}' via {args.bridge}...", file=sys.stderr, flush=True)
    candidates = survey(args.bridge, args.job)

    if not candidates:
        print("No candidates generated. Check bridge connectivity and job name.", file=sys.stderr)
        sys.exit(1)

    output = {
        "schema_version": 1,
        "job": args.job,
        "count": len(candidates),
        "candidates": candidates,
    }

    formatted = json.dumps(output, indent=2)
    if args.output:
        with open(args.output, 'w') as f:
            f.write(formatted)
            f.write('\n')
        print(f"Wrote {len(candidates)} candidates to {args.output}", file=sys.stderr)
    else:
        print(formatted)

    # Report unclassified metrics
    unclassified = [c for c in candidates if c["widget_kind"] == "unclassified"]
    if unclassified:
        print(f"\n{len(unclassified)} metrics could not be classified:", file=sys.stderr)
        for c in unclassified:
            print(f"  - {c['metric']}: {c['rule']}", file=sys.stderr)


if __name__ == "__main__":
    main()
