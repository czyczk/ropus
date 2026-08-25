## Context

`ropus` starts as an empty git repository. The reference implementation is libopus v1.6.1 at commit `22244de5a79bd1d6d623c32e72bf1954b56235be`, kept read-only in `~/src/public/opus`. Local source audio lives in `~/temp` (two FLAC music files, one MP3 speech file). Rust 1.98 is available. Motivation and scope are in `proposal.md`; normative behavior is in the four spec files.

Constraints that shape the design:

- Decoder only; encoder crates must remain straightforward to add later.
- Single-stream RFC 6716 decoding; classic PLC/DTX/CNG included; DNN/deep-PLC/DRED excluded.
- `f32` output is validated bit-exactly against the reference decoder on the same platform; deviations require root cause and user approval.
- Library crates: no `anyhow`; each module owns its `thiserror` enum; no crate-wide global error enum.
- Standard Rust across Windows/macOS/Linux on x86-64 and arm64; SIMD is feature-gated and default-on.

## Goals / Non-Goals

**Goals:**

- A faithful, reviewable transcription whose module tree mirrors the reference `celt/`, `silk/`, and `src/` organization.
- Differential validation that can pin down any non-bit-exact output to a specific module.
- A corpus and harness simple enough to re-run in CI and on every target platform.
- Performance work that is evidence-driven and provably correctness-neutral.

**Non-Goals:**

- Encoder implementation, opus_multistream/surround, DNN/deep-PLC/DRED, RTP/container demuxing, and `no_std`.
- Guaranteeing bit-identity with a reference binary built on a different compiler or platform; per-platform validation is the contract.
- Optimizing before the first full version passes differential validation (per the user's staged plan).

## Decisions

### D0: Reuse the BSD-3-Clause `ropus` port, with C isomorphism

The algorithmic base is the existing pure-Rust `ropus` codec port (BSD-3-Clause, bit-exact claims against the C reference). We vendor its codec modules, retain full attribution, remove DNN/deep-PLC/DRED and encoder surface from the decoder path, and re-target validation at the local libopus v1.6.1 reference. A committed `PORTING.md` maps every Rust module back to its C translation unit(s); when upstream libopus changes a C file, the mapping tells us which Rust file to inspect.

- **Why:** The user accepted reuse on the condition that future libopus changes remain traceable to Rust modules. An explicit mapping plus `celt/` and `silk/` trees mirroring the C file names provides that traceability without discarding a proven, bit-exact implementation.
- **Alternatives:** Hand-writing all ~96k lines (rejected by the user as unnecessary given reuse approval); using ropus as a black-box dependency without restructuring (rejected: breaks the required monorepo split and C traceability).

### D1: Workspace layout

```
crates/opus-core/            # shared types, constants, tables, math primitives
crates/opus-decoder/         # decoder library (src/celt/silk module trees)
crates/opus-decoder-cli/     # binary crate, binary name `ropusdec`
xtask/                       # corpus generation + differential harness
scripts/                     # reference build and golden reproduction
tools/                       # table extraction/verification scripts
```

- **Why:** Matches the requested bin/lib split plus a shared lib, and leaves obvious slots for future `opus-encoder`/`opus-encoder-cli` crates without renaming.
- **Alternatives:** One crate with features (rejected: no clean bin/lib boundary, feature-gated `anyhow` is awkward); separate repos (rejected: corpus and workspace validation must move together).

### D2: Module tree mirrors the reference

`opus-decoder/src` contains `packet.rs` (TOC and frame demux, mirroring `src/opus_decoder.c`), `range_decoder.rs` (mirroring `celt/entdec.c`), and `celt/` and `silk/` submodules named after the reference translation units. `opus-core/src` holds shared tables, math primitives, and entropy coding used by both modules. The public API lives in `api.rs` and models `opus_decode`/`opus_decode_float` semantics. `PORTING.md` records the C file to Rust module mapping, including consolidated upstream-ropos modules that cover several C translation units.

- **Why:** Same-structure transcription makes audits, incremental ports, and bug localization against the C source mechanical.
- **Alternative:** Redesign into idiomatic top-level modules (e.g., `imdct/`, `lpc/`) was rejected because mapping back to the reference would become lossy and error-prone.

### D3: Error ownership per module

Every module that can fail defines its own `#[derive(thiserror::Error)]` enum (`packet::Error`, `range_decoder::Error`, `celt::Error`, `silk::Error`, `pcm::Error`, ...). Module boundaries convert with `#[from]` so the source chain is preserved. The public API returns `decoder::Error`, an enum owned by the decoder module whose variants name decode-level failures and embed submodule errors; it is not a crate-wide "god enum" and does not replace the submodule types.

- **Why:** Satisfies the per-module error requirement while keeping a usable public API and full `std::error::Error::source` chains.
- **Alternatives:** A single `opus_decoder::Error` covering every module was explicitly rejected by the user. Boxing everything as `Box<dyn Error>` (rejected: loses exhaustiveness for tests). Returning a different concrete error type per module from one public function (rejected: unusable API).

### D4: Faithful numeric transcription, generated tables

CELT float math stays `f32`, SILK fixed-point math keeps its C integer widths (`i16`/`i32`/`i64`/unsigned as needed) and explicit shift semantics. Static tables are generated from the pinned reference headers by a checked-in `tools/gen_tables.py`, committed as Rust `const` arrays together with the generator and a checksum test; tables are not copied by hand.

- **Why:** Hand-copying the large SILK/CELT tables is the highest-risk source of silent bit errors. A generator makes provenance and re-generation against future reference versions mechanical.
- **Alternatives:** `include_bytes!` on converted binary blobs (rejected: less readable and worse audit); manual transcription (rejected as above).

### D5: Public decoder API

`Decoder::new(sample_rate, channels)`, `decode(&mut self, packet, out_s16, fec)`, and `decode_float(&mut self, packet, out_f32, fec)` mirror the reference entry points, but take `Option<&[u8]>` for the packet to express PLC calls, validate buffer lengths up front, and return module-owned errors instead of negative error codes. `reset()` is exposed for state re-initialization.

- **Why:** Familiar mapping to the audited C API while keeping Rust safety.
- **Alternative:** A pure streaming `impl Read` adapter (rejected: Opus state depends on packet boundaries and FEC flags; the pull model adds state-machine complexity with no validation benefit).

### D6: Output containers and conversion

The CLI writes RIFF/WAVE (s16: fmt tag 1/16-bit; s24: fmt tag 1, 24-bit packed little-endian, block align `channels*3`; f32: fmt tag 3/32-bit IEEE float) and headerless little-endian interleaved raw PCM. `f32` output is the unmodified decoder float buffer. Integer conversion duplicates the reference float-to-int path: saturate and round exactly as `opus_decode` does, then `s24` uses the same policy scaled by 2^23.

- **Why:** The user's output matrix is `{playable WAV, raw} x {s16, s24, f32}`; little-endian raw matches the reference tooling convention and keeps golden comparison byte-based.
- **Alternative:** 24-bit-in-32-bit container WAV (rejected: not "playable packed s24" per the requirement). Big-endian raw (rejected: breaks byte comparison with reference tool outputs).

### D7: Corpus and golden regeneration pipeline

1. `ffmpeg` decodes each local source to 48 kHz s16le raw and trims a 15–30 s excerpt (music) or speech passage.
2. The pinned reference `opus_demo` encodes the matrix: music at high bitrates for CELT-only frames; speech at low bitrates for SILK and mid bitrates for hybrid; DTX on/off; mono/stereo; 20 ms plus 5/10/40/60 ms frame sizes; several bitrates.
3. Only anonymized `.opus` files and a `corpus-manifest.json` are committed. Feature names look like `music-a-stereo-48k-096k.opus` or `speech-a-mono-16k-dtx.opus`.
4. `scripts/build-reference.sh` builds libopus v1.6.1 out-of-tree with `--disable-dred --disable-deep-plc --disable-osce --enable-extra-programs` and opus-tools v0.2 against that libopus so `opusdec` is pinned to the same core. `scripts/reproduce-golden.sh` regenerates golden `f32` PCM outside git. Development may use `opus_demo -d` for quick checks, but the documented golden path uses pinned `opusdec`.
5. `xtask/verify` decodes each case with the Rust decoder and compares `f32` PCM bit-by-bit against the golden stream.

- **Why:** The user required `.opus` files self-contained in git, golden audio reproducible but not committed, anonymous feature names, and reference-vs-Rust comparison with `f32` bit-exactness as the first-class target.
- **Alternatives:** Committing WAV golden files (rejected by the user: too large). Comparing only after float→int conversion (rejected: hides float differences).

### D8: Reference builds for comparison

The reference libopus is built twice on x86-64: once with default SIMD/intrinsics (the "production" reference) and once with `--disable-asm --disable-intrinsics --disable-rtcd` (the "pure C" reference). Differential validation compares the Rust scalar build against the pure-C reference and the SIMD-enabled build against the production reference; any divergence between the two references is reported to the user rather than silently accepted.

- **Why:** The C reference's SIMD paths are known to use different precision than its C paths in places; separating them tells us whether a Rust deviation is a port bug or a legitimate precision improvement.
- **Alternative:** Compare only against the default build (rejected: conflates transcription bugs with reference SIMD differences).

### D9: SIMD feature strategy

A single `simd` feature (default on) selects target-specific modules. x86-64 uses `std::arch` intrinsics with `is_x86_feature_detected!` runtime dispatch where required; aarch64 uses Neon via `std::arch` with compile-time cfg; `wasm32` uses `core::arch::wasm32` SIMD128 only where the ported ropus kernels benefit and scalar correctness passes otherwise. Optimization priority is SILK/CELT decode paths; all other code must compile and remain correct even if SIMD is skipped there. Scalar code stays the mandatory fallback. No external assembly or nightly-only intrinsics; unsafe is confined to SIMD modules and exercised by invariance tests.

- **Why:** Standard Rust, portable, feature-overridable per the requirements, and correct on CPUs without the selected extensions.
- **Alternatives:** Build-time-only `-C target-cpu=native` (rejected: binaries would crash on older CPUs); external C/asm shims (rejected: breaks "standard Rust" portability goal).

### D10: Verification and performance tooling

Unit tests live next to modules and lock risky behavior (range coder, SILK LPC/NLSF, CELT cwrs/dequant/IMDCT, PLC state). `xtask/verify` is the differential gate. Performance uses `criterion` for throughput and `cargo-show-asm` for hotspot evidence; the same CLI-based benchmark measures the reference decoder so both pay identical I/O costs.

## Risks / Trade-offs

- **Float bit-exactness across compilers/platforms:** Rust and C compilers may contract or schedule FP differently. → Compare same-platform, keep both builds at default FP settings, and treat reference-vs-reference divergences as user-review items rather than port bugs.
- **Large table and DSP transcription surface:** Silent bit errors can hide in constants and shift widths. → Generator provenance, checksum tests, and per-module differential units before full integration.
- **PLC/DTX statefulness:** These paths are hard to exercise with single isolated packets. → Dedicated packet-drop sequences in the corpus harness, not only clean decodes.
- **Unsafe SIMD code:** Intrinsic misuse can corrupt output or crash. → Invariance tests run the full corpus in both builds; unsafe is isolated and reviewed.
- **Scope creep toward encoder or multistream:** The module tree makes them tempting. → Non-goals above; encoder/multistream stay future changes with their own openspec artifacts.
- **Repository size:** Trimmed excerpts plus `.opus`-only storage is small, but the manifest must stay accurate or coverage claims rot. → Manifest validation is part of `xtask/verify`.

## Migration Plan

None: greenfield repository. First commit lands the openspec change documents, license attribution, and workspace skeleton; corpus scripts follow before any decoder implementation.

## Open Questions

None. Decisions that would alter specs, approach, or tasks have been resolved above.
