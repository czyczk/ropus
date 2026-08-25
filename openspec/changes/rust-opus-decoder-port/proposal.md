## Why

We need a pure-Rust, cross-platform Opus decoder whose audio output matches the reference libopus decoder closely enough to be trusted, without FFI bindings to the C implementation. This project transcribes the RFC 6716 decoder (CELT + SILK) from libopus v1.6.1 into a Rust monorepo and proves correctness with a differential harness and a self-contained corpus.

## What Changes

- Create a Cargo monorepo with a shared core library, a decoder library, and a decoder CLI binary; encoder crates stay planned but are not implemented in this change.
- Reuse the BSD-3-Clause pure-Rust `ropus` codec port as the algorithmic base (full attribution retained), reorganized so the module tree stays structurally isomorphic to the C reference (`celt/*.c`, `silk/*.c`, `src/*.c`) with an explicit C-to-Rust mapping document for future upstream-version synchronization.
- Implement a single-stream RFC 6716 Opus decoder: CELT and SILK coding modes, hybrid mode, DTX/CNG, and classic PLC/in-band FEC. Opus 1.5+ DNN-based deep-PLC/DRED error concealment is removed. Multistream/surround decoding is documented as future work only.
- Provide CLI output as playable PCM `.wav` files and headerless raw PCM, each for `s16`, packed `s24`, and `f32`.
- Add a self-contained, anonymized `.opus` test corpus generated from the local music/speech source files. Decoded golden artifacts are not committed to git; a reproduction guide regenerates them with a pinned original `opusdec`.
- Add a differential validation harness that compares our decoder's `f32` output bit-exactly against the reference decoder and root-causes any deviation for user evaluation.
- Add feature-gated SIMD acceleration (`x86-64` AVX family, `aarch64` Neon, `wasm32` SIMD128 where feasible), enabled by default and overridable, with correctness-invariance tests and benchmark evidence. Optimization effort prioritizes SILK/CELT decode paths; other code must compile and pass correctness even if its SIMD optimization is deferred.
- Enforce per-module `thiserror` error types in library crates (no crate-wide global error enum) and `anyhow` with full error chains in the CLI.
- Include BSD-3-Clause license attribution for code derived from libopus.

## Capabilities

### New Capabilities

- `opus-decoder`: The Rust decoder library's decoding behavior contract: RFC 6716 single-stream coverage, reference bit-exactness on the `f32` path, deterministic integer conversion, DTX/CNG, classic PLC/FEC, ML exclusion, structured errors, and SIMD output invariance.
- `decoder-cli`: The CLI contract for `.opus` input parity, the `{wav, raw} x {s16, s24, f32}` output matrix, file format details, deterministic conversion, and error reporting.
- `test-corpus`: The contract for the self-contained anonymized corpus, its coverage matrix, committed-vs-regenerated artifacts, and the differential validation harness.
- `performance-engineering`: The contract for feature-gated SIMD, hotspot analysis evidence, same-machine performance comparison against the reference decoder, and cross-platform handoff documentation.

### Modified Capabilities

None; this is a new repository.

## Impact

- New crates: `crates/opus-core`, `crates/opus-decoder`, `crates/opus-decoder-cli`, plus `xtask/` tooling and `scripts/`.
- Provenance: the vendored `ropus` base is BSD-3-Clause; attribution and the C-to-Rust module mapping are committed alongside the code.
- New dependencies (library): `thiserror`; CLI: `anyhow`, `clap`; validation/benchmarking: dev-dependencies such as `criterion` and a differential harness. No C code is vendored.
- Reference tooling requirement: an out-of-tree build of libopus v1.6.1 (pinned commit `22244de5`) and opus-tools v0.2 `opusdec` for golden reproduction. These live outside the repository and are recorded in the reproduction guide.
- Legal: repository carries BSD-3-Clause attribution because decoder logic is derived from libopus.
- Verification workflow: `cargo test`, corpus differential runs, `cargo bench`, and `cargo-show-asm` hotspot reports become part of the release checklist.
