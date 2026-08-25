# opus-decoder Specification

## Purpose
Defines the observable decoding behavior of the Rust Opus decoder library, including format coverage, reference equivalence, error handling, and feature-build invariance.

## Requirements

### Requirement: RFC 6716 single-stream decoding

The decoder SHALL decode valid RFC 6716 single-stream Opus packets for 1 or 2 channels at sampling rates 8000, 12000, 16000, 24000, and 48000 Hz, covering SILK, hybrid, and CELT coding modes and all defined frame durations (2.5, 5, 10, 20, 40, 60 ms). Multistream/surround decoding is not required.

#### Scenario: Decode a CELT-coded stereo packet

- **WHEN** the decoder processes a valid 48 kHz stereo packet whose frames use CELT-only mode
- **THEN** it returns the expected sample count and float samples that match the reference decoder for that packet

#### Scenario: Decode a SILK-coded mono packet

- **WHEN** the decoder processes a valid 48 kHz mono speech packet whose frames use SILK-only mode and the decoder is configured for 16 kHz output
- **THEN** it returns the expected sample count and float samples that match the reference decoder at 16 kHz

#### Scenario: Decode a hybrid-mode packet

- **WHEN** the decoder processes a valid packet whose frames use hybrid SILK+CELT mode
- **THEN** it returns the expected sample count and float samples that match the reference decoder

#### Scenario: Decode every standardized frame duration

- **WHEN** the committed corpus contains valid packets of each standardized frame duration
- **THEN** the decoder accepts every one of them and produces the reference-matching output

### Requirement: Bit-exact float output against reference

The decoder's float output SHALL be bit-identical to the pinned reference libopus v1.6.1 decoder built with the matching fixed-point 16-bit configuration (`--enable-fixed-point --disable-fixed-res24`, ML paths disabled) for the committed corpus on the same platform. The default float reference build SHALL be decoded alongside and reported; differences between the two reference configurations are precision-floor findings, not algorithm bugs, and SHALL be documented and approved by the user.

#### Scenario: Corpus differential run is clean

- **WHEN** the differential harness decodes every committed `.opus` case with the Rust decoder and the reference decoder
- **THEN** the harness reports zero bit differences between the two `f32` PCM streams

#### Scenario: A deviation is observed

- **WHEN** any case differs by one or more bits
- **THEN** the cause MUST be root-caused, documented with an assessment of which implementation is more accurate, and approved by the user before the case is accepted

### Requirement: Deterministic integer conversion

The decoder SHALL provide deterministic `s16` output using the same saturation and rounding behavior as the reference `opus_decode` path. `s24` conversion SHALL use packed 24-bit signed samples scaled from the same float values with the same saturation policy.

#### Scenario: Full-scale clipping

- **WHEN** a decoded float sample equals or exceeds the clipping boundary
- **THEN** the `s16` and `s24` outputs saturate to the maximum or minimum representable value exactly as the reference conversion does

#### Scenario: Integer output matches reference

- **WHEN** the corpus is decoded to `s16` by the Rust decoder and by the reference decoder using the same conversion path
- **THEN** the `s16` PCM streams are byte-identical

### Requirement: DTX and comfort noise decoding

The decoder SHALL decode discontinuous-transmission packets and generate comfort noise with the same samples as the reference decoder.

#### Scenario: DTX packet produces comfort noise

- **WHEN** the decoder receives a DTX packet from a corpus case encoded with DTX enabled
- **THEN** it returns non-silent comfort-noise samples that match the reference decoder for that packet sequence

### Requirement: Classic PLC and in-band FEC

The decoder SHALL implement RFC 6716 packet loss concealment and decode in-band FEC when requested, matching the reference decoder's classic PLC behavior. Deep-PLC/DNN concealment SHALL NOT be used.

#### Scenario: Lost packet is concealed

- **WHEN** the differential harness drops a packet from a committed case and calls both decoders in PLC mode
- **THEN** the Rust decoder's concealed output matches the reference decoder's classic PLC output

#### Scenario: In-band FEC is decoded

- **WHEN** a packet contains in-band FEC and the decoder is asked to decode FEC
- **THEN** the returned FEC audio matches the reference decoder's FEC decode for that packet

### Requirement: No machine-learning concealment paths

The implementation SHALL NOT contain or call DNN/deep-PLC/DRED code paths, and the reference build used for comparison SHALL be configured with those paths disabled.

#### Scenario: Reference and Rust builds exclude ML paths

- **WHEN** the reference configure command and the Rust feature list are inspected
- **THEN** deep-PLC, DRED, and related ML options are disabled/absent, and this configuration is recorded in the validation documentation

### Requirement: Structured errors instead of panics

The decoder SHALL reject malformed input by returning a typed, context-preserving error from the module that owns the failure. It SHALL NOT panic on malformed packet data, truncated buffers, or corrupt entropy-coded symbols.

#### Scenario: Truncated packet

- **WHEN** the decoder is given a truncated `.opus` packet
- **THEN** it returns a structured error whose error chain identifies the failing module, and the process does not panic

#### Scenario: Corrupt range-coded data

- **WHEN** a crafted packet makes the range decoder read past the valid payload
- **THEN** the decoder returns the range decoder's own error type and no undefined behavior or panic occurs

### Requirement: SIMD build invariance

Enabling or disabling the SIMD cargo feature SHALL NOT change any decoded sample or any observable API behavior.

#### Scenario: Scalar and SIMD builds produce identical output

- **WHEN** the same corpus is decoded by builds with the SIMD feature enabled and disabled
- **THEN** both builds produce byte-identical `f32`, `s16`, and `s24` outputs for every case
