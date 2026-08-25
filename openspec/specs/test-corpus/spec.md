# test-corpus Specification

## Purpose
Defines the self-contained validation corpus built from the local music and speech sources, what is committed to git, how golden audio is reproduced, and how differential validation runs.

## Requirements

### Requirement: Self-contained anonymized corpus in git

The repository SHALL contain every validation `.opus` file needed to run the full differential suite. Names SHALL encode only technical features (source class, channel count, sample rate, bitrate, mode/DTX, frame size) and SHALL NOT reveal original track titles or artist names.

#### Scenario: Clean checkout has all cases

- **WHEN** the corpus directory is listed on a clean checkout
- **THEN** it contains only anonymized `.opus` files plus their manifest, and the manifest maps each feature name to its generation parameters

#### Scenario: No source audio or golden audio committed

- **WHEN** `git ls-files` is run
- **THEN** no `.flac`, `.mp3`, decoded `.wav`, or raw golden PCM file is listed

### Requirement: Coverage matrix

The corpus SHALL cover, at minimum: CELT-only decoding via music at multiple bitrates, SILK-only and hybrid decoding via speech at multiple bitrates, mono and stereo, DTX on/off, 20 ms and at least one non-20 ms frame duration, and multiple packet bitrates spanning the standardized range.

#### Scenario: Matrix manifest is complete

- **WHEN** the corpus manifest is validated
- **THEN** each required coverage class has at least one case and the validator reports success

### Requirement: Golden reproduction with pinned original opusdec

A reproduction guide and script SHALL regenerate golden decoded audio outside git using the pinned original decoder (`opus_demo` from libopus v1.6.1, with `opusdec` from opus-tools v0.2 recorded for Ogg files). The guide SHALL record exact revisions, configure flags, and invocation so results are reproducible.

#### Scenario: Golden regeneration is reproducible

- **WHEN** the guide is followed on a machine with the pinned toolchain
- **THEN** the regenerated golden files are byte-identical to the files used during development validation

### Requirement: Differential validation harness

The repository SHALL provide a harness that decodes every committed corpus case with both the Rust decoder and the reference decoder and compares the `f32` PCM bit-by-bit, reporting case names, bit-difference counts, and a clean overall verdict.

#### Scenario: Harness passes on a known-good build

- **WHEN** the harness is run against a build that matches the reference behavior
- **THEN** it exits 0 and reports zero bit differences for all cases

#### Scenario: Harness fails informatively

- **WHEN** any case differs
- **THEN** the harness exits non-zero and names the failing feature, frame position, and first differing sample so the defect can be investigated
