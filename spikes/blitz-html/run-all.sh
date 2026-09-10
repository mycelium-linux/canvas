#!/usr/bin/env bash
# Everything the spike needs, under ONE lockout slot: the binary is built once,
# then every experiment runs against it.
set -u
cd "$(dirname "$0")"
export CARGO_TARGET_DIR="$PWD/target"
B() { cargo run --offline -q -- "$@"; }

echo "############ BUILD ############"
cargo build --offline 2>&1 | tail -5
rc=${PIPESTATUS[0]}
cargo build --offline > /dev/null 2>&1; rc=$?
echo "cargo build exit: $rc"
[ $rc -eq 0 ] || { echo "BUILD FAILED - aborting"; exit $rc; }

echo; echo "############ 1. REAL TEMPLATE: A6 THERMAL PACKING SLIP ############"
B render; echo "render exit: $?"

echo; echo "############ 2. REAL TEMPLATE: A4 INVOICE (multi-page) ############"
B invoice; echo "invoice exit: $?"

echo; echo "############ 3. CSS FEATURE PROBES ############"
B probe; echo "probe exit: $?"

echo; echo "############ 4. CSS PROBE CORPUS (same file WeasyPrint rendered) ############"
# 4in page, 0.15in margins => 3.7in = 355.2 CSS px content box
B file fixtures/css-probe.html out/css-probe.blitz.png 355 2; echo "file exit: $?"

echo; echo "############ 5. WEASYPRINT ON THE IDENTICAL HTML ############"
./compare.sh blitz  || echo "compare.sh blitz failed"
echo
./compare.sh invoice || echo "compare.sh invoice failed"

echo; echo "############ 6. STRUCTURAL COMPARISON ############"
B compare blitz; echo "compare exit: $?"

echo; echo "############ ARTIFACTS ############"
ls -la out/
