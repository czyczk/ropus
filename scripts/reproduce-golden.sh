#!/usr/bin/env bash
# Regenerate golden f32 PCM for the committed corpus with the pinned original
# decoder. Golden files are deliberately NOT committed; they are written under
# ${ROPUS_GOLDEN:-.refbuild/golden} and can be recreated byte-for-byte.
#
# Usage:
#   scripts/reproduce-golden.sh [--fixed16|--prod]   (default: --fixed16)
#
# --fixed16 is the primary bit-exact oracle for the fixed-point Rust core.
# --prod is the default libopus v1.6.1 float build, kept for deviation reports.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REFBUILD="${ROPUS_REFBUILD:-$ROOT/.refbuild}"
GOLDEN="${ROPUS_GOLDEN:-$ROOT/.refbuild/golden}"
MODE="${1:---fixed16}"

case "$MODE" in
  --fixed16) DEMO="$REFBUILD/opus-src-fixed16/opus_demo" ;;
  --prod)    DEMO="$REFBUILD/opus-src/opus_demo" ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac

if [ ! -x "$DEMO" ]; then
  echo "reference decoder not found; run scripts/build-reference.sh first" >&2
  exit 1
fi

mkdir -p "$GOLDEN"
python3 - "$ROOT/corpus/manifest.json" "$GOLDEN" "$DEMO" <<'PY'
import json, subprocess, sys
from pathlib import Path

manifest_path, golden, demo = sys.argv[1], Path(sys.argv[2]), sys.argv[3]
golden.mkdir(parents=True, exist_ok=True)
manifest = json.loads(Path(manifest_path).read_text())
for e in manifest["entries"]:
    src = Path(manifest_path).parent / e["file"]
    dst = golden / f"{e['id']}.f32"
    cmd = [demo, "-d", "48000", str(e["channels"]), "-f32", str(src), str(dst)]
    rc = subprocess.run(cmd).returncode
    if rc != 0:
        sys.exit(f"reference decode failed for {e['id']}: {cmd}")
    print(f"ok {dst.relative_to(golden.parent)}")
PY
