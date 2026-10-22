#!/usr/bin/env bash
# Guard rail against accidental iced default-feature enablement.
#
# Cargo unifies features ADDITIVELY across the dependency graph. If any
# dependency declares `default-features = false` on its iced-family dep but
# another crate enables iced's defaults, the default features sneak back in:
#
#   - web-colors: flips GAMMA_CORRECTION off, breaking all compositing
#   - crisp:    flips snap globally, changing text rendering
#   - tiny-skia: compiles a second renderer backend (OOM on memory-constrained hosts)
#
# This check inspects the resolved feature graph and fails loudly if any
# iced-family default feature is enabled when Canvas did not ask for it.
#
# Usage:
#   sh scripts/check-iced-features.sh
#
# Exit codes:
#   0 — no unwanted features found
#   1 — one or more unwanted iced features are enabled in the resolved graph

set -euo pipefail

# Feature names that Canvas does NOT enable (from AGENTS.md and Cargo.toml):
# canvas:   wgpu, tokio, canvas, image-without-codecs, x11, wayland, markdown, highlighter
# canvas-mac: tiny-skia, tokio, image-without-codecs, markdown
# canvas-widgets: wgpu, canvas, tokio (via live feature)
#
# Unwanted iced defaults: web-colors, crisp, tiny-skia (on canvas, not canvas-mac)
UNWANTED_FEATURES="web-colors,crisp,tiny-skia"

echo "Checking iced feature graph for unwanted defaults..."
echo ""

# Get the feature tree for the canvas crate (not canvas-mac, which legitimately uses tiny-skia)
FEATURE_TREE=$(cargo tree -e features -p canvas 2>/dev/null || true)

# Check for each unwanted feature
FAILED=0
for feature in web-colors crisp tiny-skia; do
    # Look for the feature enabled on iced_graphics or iced_wgpu
    MATCHES=$(echo "$FEATURE_TREE" | grep -E "iced[-_][^ ]* v[0-9].*$feature" || true)
    if [ -n "$MATCHES" ]; then
        echo "FAIL: iced family member has unwanted feature '$feature' enabled:"
        echo "$MATCHES"
        echo ""
        FAILED=1
    fi
done

if [ "$FAILED" -eq 1 ]; then
    echo "ERROR: Unwanted iced default features detected."
    echo "       These flip GAMMA_CORRECTION, snap, or compile a second renderer."
    echo "       Check that no dependency enables iced's default features."
    echo ""
    echo "To debug:"
    echo "  cargo tree -e features -p canvas | grep -E '$UNWANTED_FEATURES'"
    exit 1
fi

echo "OK: no unwanted iced default features in the canvas feature graph."
