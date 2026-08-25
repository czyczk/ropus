#!/usr/bin/env python3
"""Differential corpus verification.

Compares `ropusdec` f32 output bit-by-bit against:
  - the fixed-point 16-bit reference libopus v1.6.1 decoder (primary target), and
  - the default float reference libopus v1.6.1 decoder (reported, not gating).

Usage:
  scripts/verify-corpus.py --decoder <ropusdec> --fixed16 <opus_demo> --prod <opus_demo>
"""
import argparse
import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "corpus"


def run(cmd):
    return subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode


def read_f32(path):
    data = Path(path).read_bytes()
    return struct.unpack(f"<{len(data)//4}f", data)


def diff_stats(a, b):
    n = min(len(a), len(b))
    diffs = [i for i in range(n) if a[i] != b[i]]
    max_abs = max((abs(a[i] - b[i]) for i in diffs), default=0.0)
    return len(a), len(b), len(diffs), max_abs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--decoder", required=True)
    ap.add_argument("--fixed16", required=True)
    ap.add_argument("--prod", required=True)
    ap.add_argument("--out", default=str(ROOT / ".refbuild" / "validation-report.json"))
    args = ap.parse_args()

    manifest = json.loads((CORPUS / "manifest.json").read_text())
    work = ROOT / ".refbuild" / "verify-work"
    work.mkdir(parents=True, exist_ok=True)

    results = []
    failed = []
    for e in manifest["entries"]:
        src = CORPUS / e["file"]
        rust = work / f"{e['id']}.rust.f32"
        fixed16 = work / f"{e['id']}.fixed16.f32"
        prod = work / f"{e['id']}.prod.f32"
        ch = e["channels"]

        rc = run([args.decoder, "--channels", str(ch), "--rate", "48000",
                  "--output-type", "raw", "--sample-format", "f32", str(src), str(rust)])
        if rc != 0:
            failed.append((e["id"], "ropusdec failed", rc))
            continue
        rc = run([args.fixed16, "-d", "48000", str(ch), "-f32", str(src), str(fixed16)])
        if rc != 0:
            failed.append((e["id"], "fixed16 reference failed", rc))
            continue
        rc = run([args.prod, "-d", "48000", str(ch), "-f32", str(src), str(prod)])
        if rc != 0:
            failed.append((e["id"], "prod reference failed", rc))
            continue

        a = read_f32(rust)
        b16 = read_f32(fixed16)
        bp = read_f32(prod)
        n_a, n_b16, d16, m16 = diff_stats(a, b16)
        _, n_bp, dp, mp = diff_stats(a, bp)
        row = {
            "id": e["id"],
            "mode": e["mode"],
            "frame_ms": e["frame_ms"],
            "channels": ch,
            "rust_samples": n_a,
            "fixed16_samples": n_b16,
            "fixed16_bit_diffs": d16,
            "fixed16_max_abs": m16,
            "prod_samples": n_bp,
            "prod_bit_diffs": dp,
            "prod_max_abs": mp,
        }
        results.append(row)
        status = "OK " if d16 == 0 and n_a == n_b16 else "FAIL"
        print(f"{status} {e['id']:<32} fixed16_diffs={d16:>6} prod_diffs={dp:>6} prod_max_abs={mp:.3e}")
        if d16 != 0 or n_a != n_b16:
            failed.append((e["id"], "fixed16 bit mismatch", d16))

    report = {
        "decoder": args.decoder,
        "fixed16_reference": args.fixed16,
        "prod_reference": args.prod,
        "results": results,
        "failed": [{"id": f[0], "reason": f[1], "detail": f[2]} for f in failed],
    }
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"\n{len(results)} cases compared, {len(failed)} failures; report: {args.out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
