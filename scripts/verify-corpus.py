#!/usr/bin/env python3
"""Differential corpus verification.

For every committed corpus case, compares `ropusdec` output against:
  - the fixed-point 16-bit reference libopus v1.6.1 decoder (primary oracle,
    gating), for f32, s16, and s24 raw outputs;
  - the default float reference libopus v1.6.1 decoder (f32, reported).

The manifest is validated first: every entry must exist, its SHA-256 must
match, and the required coverage classes must be present.

Usage:
  scripts/verify-corpus.py --decoder <ropusdec> --fixed16 <opus_demo> --prod <opus_demo>
"""
import argparse
import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "corpus"

REQUIRED_COVERAGE = [
    ("mode", "CELT"),
    ("mode", "SILK"),
    ("mode", "HYBRID"),
    ("channels", 1),
    ("channels", 2),
    ("frame_ms", 5),
    ("frame_ms", 10),
    ("frame_ms", 20),
    ("frame_ms", 40),
    ("frame_ms", 60),
    ("frame_ms", 120),
    ("dtx", True),
    ("inband_fec", True),
]


def run(cmd):
    return subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode


def read_f32(path):
    data = Path(path).read_bytes()
    return struct.unpack(f"<{len(data)//4}f", data)


def diff_stats_bytes(a, b):
    n = min(len(a), len(b))
    diffs = sum(1 for i in range(n) if a[i] != b[i])
    return len(a), len(b), diffs


def diff_stats_f32(a, b):
    n = min(len(a), len(b))
    diffs = [i for i in range(n) if a[i] != b[i]]
    max_abs = max((abs(a[i] - b[i]) for i in diffs), default=0.0)
    return len(a), len(b), len(diffs), max_abs


def validate_manifest(manifest):
    errors = []
    for e in manifest["entries"]:
        path = CORPUS / e["file"]
        if not path.is_file():
            errors.append(f"{e['id']}: missing {path}")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != e.get("sha256"):
            errors.append(f"{e['id']}: sha256 mismatch")
    for key, value in REQUIRED_COVERAGE:
        if not any(e.get(key) == value for e in manifest["entries"]):
            errors.append(f"coverage missing: {key}={value}")
    return errors


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--decoder", required=True)
    ap.add_argument("--fixed16", required=True)
    ap.add_argument("--prod", required=True)
    ap.add_argument("--out", default=str(ROOT / ".refbuild" / "validation-report.json"))
    args = ap.parse_args()

    manifest = json.loads((CORPUS / "manifest.json").read_text())
    errors = validate_manifest(manifest)
    if errors:
        print("manifest validation FAILED:")
        for e in errors:
            print(" ", e)
        return 1

    work = ROOT / ".refbuild" / "verify-work"
    work.mkdir(parents=True, exist_ok=True)

    results = []
    failed = []
    for e in manifest["entries"]:
        src = CORPUS / e["file"]
        ch = e["channels"]
        row = {
            "id": e["id"],
            "mode": e["mode"],
            "frame_ms": e["frame_ms"],
            "channels": ch,
        }

        # Primary oracle: f32 bit-exactness.
        rust_f32 = work / f"{e['id']}.rust.f32"
        fixed16_f32 = work / f"{e['id']}.fixed16.f32"
        prod_f32 = work / f"{e['id']}.prod.f32"
        if run([args.decoder, "--channels", str(ch), "--rate", "48000",
                "--output-type", "raw", "--sample-format", "f32", str(src), str(rust_f32)]) != 0:
            failed.append((e["id"], "ropusdec f32 failed", None))
            continue
        if run([args.fixed16, "-d", "48000", str(ch), "-f32", str(src), str(fixed16_f32)]) != 0:
            failed.append((e["id"], "fixed16 f32 reference failed", None))
            continue
        if run([args.prod, "-d", "48000", str(ch), "-f32", str(src), str(prod_f32)]) != 0:
            failed.append((e["id"], "prod f32 reference failed", None))
            continue

        a = read_f32(rust_f32)
        b16 = read_f32(fixed16_f32)
        bp = read_f32(prod_f32)
        n_a, n_b16, d16, m16 = diff_stats_f32(a, b16)
        _, n_bp, dp, mp = diff_stats_f32(a, bp)
        row.update({
            "rust_samples": n_a,
            "fixed16_samples": n_b16,
            "fixed16_bit_diffs": d16,
            "fixed16_max_abs": m16,
            "prod_samples": n_bp,
            "prod_bit_diffs": dp,
            "prod_max_abs": mp,
        })
        status = "OK " if d16 == 0 and n_a == n_b16 else "FAIL"
        print(f"{status} {e['id']:<32} f32_fixed16_diffs={d16:>6} f32_prod_diffs={dp:>6}")
        if d16 != 0 or n_a != n_b16:
            failed.append((e["id"], "fixed16 f32 bit mismatch", d16))

        # Secondary oracles: s16 and s24 byte-exactness against fixed16.
        for fmt, ref_flag in (("s16", "-16"), ("s24", "-24")):
            rust = work / f"{e['id']}.rust.{fmt}"
            ref = work / f"{e['id']}.fixed16.{fmt}"
            if run([args.decoder, "--channels", str(ch), "--rate", "48000",
                    "--output-type", "raw", "--sample-format", fmt, str(src), str(rust)]) != 0:
                failed.append((e["id"], f"ropusdec {fmt} failed", None))
                continue
            if run([args.fixed16, "-d", "48000", str(ch), ref_flag, str(src), str(ref)]) != 0:
                failed.append((e["id"], f"fixed16 {fmt} reference failed", None))
                continue
            n_r, n_f, diffs = diff_stats_bytes(rust.read_bytes(), ref.read_bytes())
            row[f"{fmt}_byte_diffs"] = diffs
            if diffs != 0 or n_r != n_f:
                failed.append((e["id"], f"fixed16 {fmt} byte mismatch", diffs))
        results.append(row)

    report = {
        "decoder": args.decoder,
        "fixed16_reference": args.fixed16,
        "prod_reference": args.prod,
        "manifest_ok": not errors,
        "results": results,
        "failed": [{"id": f[0], "reason": f[1], "detail": f[2]} for f in failed],
    }
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"\n{len(results)} cases compared, {len(failed)} failures; report: {args.out}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
