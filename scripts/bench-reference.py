#!/usr/bin/env python3
"""Same-machine throughput comparison: ropusdec vs reference opus_demo.

Both binaries decode the committed corpus to f32 raw and write to /dev/null,
so process setup, file I/O, and output write costs are identical in kind.
Reports median wall time and the Rust/reference ratio (lower is faster).

Usage:
  scripts/bench-reference.py --decoder target/release/ropusdec \
    --fixed16 .refbuild/opus-src-fixed16/opus_demo --iters 10
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASES = [
    ("music-a-celt-096k-20ms.opus", 2),
    ("speech-silk-012k-20ms.opus", 1),
    ("speech-hybrid-032k-20ms.opus", 1),
    ("speech-silk-012k-dtx-20ms.opus", 1),
]
DEVNULL = os.devnull


def run(cmd):
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


def median_time(cmd_fn, iters):
    samples = []
    for _ in range(iters):
        t0 = time.perf_counter()
        cmd_fn()
        samples.append(time.perf_counter() - t0)
    return statistics.median(samples)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--decoder", required=True)
    ap.add_argument("--fixed16", required=True)
    ap.add_argument("--iters", type=int, default=10)
    ap.add_argument("--out", default=str(ROOT / ".refbuild" / "bench-report.json"))
    args = ap.parse_args()

    rows = []
    for file, channels in CASES:
        src = ROOT / "corpus" / file

        def rust():
            run([args.decoder, "--channels", str(channels), "--rate", "48000",
                 "--output-type", "raw", "--sample-format", "f32", str(src), DEVNULL])

        def cref():
            run([args.fixed16, "-d", "48000", str(channels), "-f32", str(src), DEVNULL])

        rust_t = median_time(rust, args.iters)
        ref_t = median_time(cref, args.iters)
        ratio = rust_t / ref_t if ref_t > 0 else float("inf")
        row = {"file": file, "channels": channels, "rust_median_s": rust_t,
               "reference_median_s": ref_t, "ratio": ratio}
        rows.append(row)
        print(f"{file:<34} rust={rust_t*1000:8.2f} ms  ref={ref_t*1000:8.2f} ms  ratio={ratio:.3f}")

    report = {"decoder": args.decoder, "reference": args.fixed16, "iters": args.iters, "rows": rows}
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    overall = statistics.mean(r["ratio"] for r in rows)
    print(f"\nmean ratio {overall:.3f} (<1 means Rust faster); report: {args.out}")
    return 0 if overall <= 1.0 else 1


if __name__ == "__main__":
    sys.exit(main())
