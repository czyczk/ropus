## Purpose

Defines the performance-engineering contract: feature-gated SIMD for x86-64 and aarch64, output invariance, evidence-based hotspot analysis, same-machine comparison with the reference decoder, and cross-platform handoff.

## ADDED Requirements

### Requirement: Feature-gated SIMD

The decoder SHALL expose a `simd` cargo feature that is enabled by default and can be overridden with `default-features = false`. With the feature enabled, x86-64 builds SHALL use available SIMD paths, aarch64 builds SHALL use Neon where beneficial, and wasm32 builds SHALL compile with SIMD128 enabled; all builds SHALL retain a portable scalar fallback. SILK and CELT decode paths are the optimization priority; other code MAY remain scalar as long as it compiles and passes correctness.

#### Scenario: Default and no-default builds

- **WHEN** the workspace is built with default features and with `--no-default-features`
- **THEN** both configurations compile, test, and run on their target platforms

#### Scenario: wasm32 target builds

- **WHEN** the decoder library is built for `wasm32-unknown-unknown` with and without the `simd` feature
- **THEN** both configurations compile, and the wasm correctness test decodes the committed corpus cases with the same samples as native builds

#### Scenario: Scalar fallback used when disabled

- **WHEN** the `simd` feature is disabled on x86-64 or aarch64
- **THEN** decoding succeeds without executing feature-specific SIMD instructions

### Requirement: SIMD preserves output invariance

Enabling SIMD SHALL NOT change decoded samples relative to the scalar build for any committed corpus case.

#### Scenario: Cross-build output comparison

- **WHEN** the differential harness runs the same corpus through SIMD-enabled and SIMD-disabled builds
- **THEN** both produce byte-identical `f32` PCM for every case

### Requirement: Scientific hotspot analysis evidence

Performance work SHALL be driven by recorded profiling evidence. The repository SHALL contain or reference a `cargo-show-asm` hotspot report and benchmark output identifying the functions that dominate decode time, informed also by which functions the reference implementation optimized in assembly.

#### Scenario: Hotspot report exists before optimization

- **WHEN** a performance change is proposed after the first full implementation
- **THEN** the change references the hotspot report and explains why the targeted function is on the critical path

### Requirement: Same-machine performance comparison

The decoder SHALL be benchmarked on the same Linux x86-64 machine as the reference decoder, using the committed corpus and equivalent decode configuration, and the measured throughput SHALL be at least equal to the reference decoder's throughput.

#### Scenario: Benchmark comparison is recorded

- **WHEN** the performance comparison command is run
- **THEN** it produces a stored report with both decoders' throughput per case and an overall comparison verdict

### Requirement: Cross-platform handoff documentation

The repository SHALL include handoff documentation enabling correctness and performance verification on macOS arm64, Linux arm64, and Windows x86-64/arm64, including build commands, benchmark commands, and expected result formats.

#### Scenario: Handoff guide is complete

- **WHEN** the handoff document is reviewed
- **THEN** it contains platform-specific build, test, and benchmark instructions for all four target platforms plus a results template for the user to fill in
