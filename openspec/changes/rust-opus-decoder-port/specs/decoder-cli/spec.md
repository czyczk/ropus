## Purpose

Defines the command-line decoder's observable contract for single-stream `.opus` input, the playable WAV and raw PCM output matrix, deterministic conversion, and error reporting.

## ADDED Requirements

### Requirement: Single-stream input parity

The CLI SHALL accept any single-stream `.opus` file that the pinned reference `opusdec` accepts, including all RFC 6716 frame durations, SILK/hybrid/CELT modes, DTX, and 1 or 2 channels. It SHALL NOT accept multistream/surround files.

#### Scenario: Corpus files decode successfully

- **WHEN** the CLI is invoked on each committed corpus `.opus` file
- **THEN** it exits with status 0 and writes a valid output file

#### Scenario: Unsupported multistream input

- **WHEN** the CLI is invoked on a multistream Opus file
- **THEN** it exits non-zero with an error message that explains multistream is unsupported

### Requirement: Output format matrix

The CLI SHALL support six output combinations: `{wav, raw}` container x `{s16, s24, f32}` sample encoding. `s24` means packed little-endian 24-bit signed PCM, and `f32` means IEEE-754 32-bit float samples.

#### Scenario: s16 WAV output

- **WHEN** the CLI decodes with `wav` + `s16`
- **THEN** it writes a RIFF/WAVE file with PCM format tag 1, 16 bits per sample, and interleaved channel data

#### Scenario: s24 WAV output

- **WHEN** the CLI decodes with `wav` + `s24`
- **THEN** it writes a RIFF/WAVE file with PCM format tag 1, 24 bits per sample, block alignment of `channels * 3` bytes, and packed little-endian samples

#### Scenario: f32 WAV output

- **WHEN** the CLI decodes with `wav` + `f32`
- **THEN** it writes a RIFF/WAVE file with IEEE-float format tag 3 and 32 bits per sample

#### Scenario: Raw output

- **WHEN** the CLI decodes with the `raw` container and any supported sample encoding
- **THEN** it writes headerless little-endian interleaved PCM containing exactly the decoded samples

### Requirement: Deterministic sample conversion

The CLI's `s16` and `s24` conversion SHALL saturate and round exactly like the reference decoder's integer path, and the `f32` path SHALL write the unmodified decoder float samples.

#### Scenario: Conversions are stable across runs

- **WHEN** the same `.opus` file is decoded twice with identical flags
- **THEN** the two output files are byte-identical

#### Scenario: Integer outputs match reference conversion

- **WHEN** corpus files are decoded to `s16` by the CLI and to `s16` by the pinned reference decoder
- **THEN** the sample bytes are identical for every case

### Requirement: Error reporting with full chain

The CLI SHALL use `anyhow` to report failures with the complete source error chain and a non-zero exit code.

#### Scenario: Invalid or missing input file

- **WHEN** the CLI is invoked with a path that does not exist or is not a valid Opus packet stream
- **THEN** it exits non-zero and prints the underlying cause chain without panicking

#### Scenario: Unwritable output path

- **WHEN** the output path cannot be created
- **THEN** it exits non-zero and the printed error identifies the I/O failure
