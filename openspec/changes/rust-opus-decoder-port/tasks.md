## 1. Base Integration and Architecture

- [x] 1.1 Vendor the BSD-3-Clause ropus codec source into `crates/opus-core` with license headers and verify the crate builds standalone
- [x] 1.2 Split the decoder surface into `crates/opus-decoder` (public API + decoder modules) and the CLI into `crates/opus-decoder-cli` and verify `cargo build --workspace` succeeds
- [ ] 1.3 Remove DNN/deep-PLC/DRED modules and wire classic PLC only and verify no `dnn`, `deep-plc`, or `dred` symbols remain in the decoder path
- [ ] 1.4 Restore C-isomorphic module names under `celt/` and `silk/` (one Rust module per C translation unit where practical) and verify `PORTING.md` maps every C file to its Rust module
- [x] 1.5 Add per-module `thiserror` error enums and `#[from]` chains in `opus-decoder` and verify a chain test asserts the expected source chain
- [x] 1.6 Add CLI skeleton (`anyhow` + `clap`) with `--output-type`, `--sample-format`, input/output paths and verify `--help` documents all six output combinations

## 2. Reference Toolchain

- [x] 2.1 Commit `scripts/build-reference.sh` that builds libopus v1.6.1 (pinned commit 22244de5) with ML paths disabled and verify re-running is idempotent
- [x] 2.2 Build opus-tools v0.2 against the pinned libopus prefix and verify `opusdec --version` reports opus-tools 0.2 using libopus 1.6.1
- [x] 2.3 Build a second pure-C reference (`--disable-asm --disable-intrinsics --disable-rtcd`) and verify both reference builds report ML paths disabled
- [x] 2.4 Commit `scripts/reproduce-golden.sh` using the pinned `opusdec` and verify a second run regenerates byte-identical golden output

## 3. Corpus Generation

- [x] 3.1 Decode the two FLAC and one MP3 sources to 48 kHz s16le raw with `ffmpeg` and verify all three outputs decode without errors
- [x] 3.2 Trim a 15–30 s music excerpt per FLAC and a 15–30 s speech passage from the MP3 and verify durations are in range
- [x] 3.3 Define and commit the corpus matrix: music high-bitrate CELT, speech low-bitrate SILK, speech mid-bitrate hybrid, DTX on/off, mono/stereo, 5/10/20/40/60 ms frames, and verify `corpus-manifest.json` is complete
- [x] 3.4 Encode the matrix with reference `opus_demo` into anonymized feature-named `.opus` files and verify each file decodes with pinned `opusdec`
- [x] 3.5 Commit only `.opus` files + manifest and verify `git ls-files` contains no FLAC/MP3/WAV/raw golden artifacts
- [ ] 3.6 Add manifest validation to `xtask` and verify it reports success for the committed corpus

## 4. Decoder API and Output Paths

- [x] 4.1 Expose `Decoder::new(sample_rate, channels)`, `decode`, and `decode_float` from `opus-decoder` and verify API unit tests cover buffer-length and channel validation
- [x] 4.2 Implement float-to-s16/s24 conversion with reference saturation/rounding and verify clipping-boundary and rounding unit tests
- [x] 4.3 Implement WAV header writing and raw interleaved output helpers and verify header bytes for s16/s24/f32 against expected values
- [x] 4.4 Wire the six CLI output combinations through `anyhow` error chains and verify each writes a playable/headerless file for a representative case

## 5. Differential Validation

- [x] 5.1 Implement `xtask/verify` reading the manifest and comparing f32 PCM bit-by-bit against golden output and verify the harness reports zero differences on the full corpus
- [x] 5.2 Run the corpus through both the pure-C and production reference decoders and verify the Rust decoder agrees with both, documenting any reference-vs-reference divergence for user evaluation
- [x] 5.3 Validate s16 outputs byte-identical to the reference integer path for every corpus case and s24 outputs against the documented conversion rule
- [x] 5.4 Run the CLI's six output combinations across the corpus and verify WAV headers, raw lengths, and exit codes
- [x] 5.5 Produce and commit the validation report (case table, bit differences, build flags) and verify it is reproducible from a clean checkout
- [x] 5.6 Stop at the user listening checkpoint and verify the built CLI plus reference decoder are available for audition

## 6. Code Quality Review

- [ ] 6.1 Review the tree for workarounds, non-idiomatic Rust, and unsafe usage and verify each finding is recorded with an owner
- [ ] 6.2 Apply approved refactors and verify `cargo test --workspace` and the full differential run stay clean
- [ ] 6.3 Re-audit error types per module and public API signatures and verify no `anyhow` appears in library crates

## 7. Performance, SIMD, and wasm

- [ ] 7.1 Add criterion benchmarks for representative CELT, SILK, hybrid, and DTX cases and verify baseline throughput numbers are recorded
- [ ] 7.2 Produce the cargo-show-asm hotspot report and cross-reference reference asm usage and verify top functions are documented before optimization
- [ ] 7.3 Optimize SILK/CELT decode hotspots for x86-64 behind the `simd` feature and verify SIMD-enabled output is bit-identical to scalar on the full corpus
- [ ] 7.4 Add aarch64 Neon paths for the same hotspots and verify cross-compilation plus invariance on available ARM hardware or emulation
- [ ] 7.5 Add `wasm32-unknown-unknown` builds with and without SIMD128 and verify wasm decode matches native output on the corpus
- [ ] 7.6 Benchmark Rust vs reference decoder on the same machine with identical I/O and verify the stored report shows Rust throughput >= reference
- [ ] 7.7 Write cross-platform handoff documentation for macOS arm64, Linux arm64, Windows x86-64/arm64, and wasm and verify it contains build, test, benchmark, and results-template sections

## 8. Final Review and Archive

- [ ] 8.1 Review RFC 6716 coverage and C-to-Rust isomorphism against the reference decoder feature set and verify the coverage checklist is committed
- [ ] 8.2 Final code elegance and safety review pass and verify findings are resolved without changing validation results
- [ ] 8.3 Update README with build, validation, benchmark, provenance, and handoff pointers and verify every documented command runs as written
- [ ] 8.4 Run `openspec validate rust-opus-decoder-port --strict` and the full test/bench suite on a clean checkout and verify everything passes
