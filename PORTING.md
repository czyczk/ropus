# C-to-Rust porting map

Reference: libopus **v1.6.1**, commit `22244de5a79bd1d6d623c32e72bf1954b56235be`.
Base port: [0x4D44/ropus](https://github.com/0x4D44/ropus) (BSD-3-Clause, vendored
into `crates/opus-core`). When upstream libopus changes a C file, start from the
Rust module listed here. The base port groups several C translation units into
one Rust module; the grouping is intentional and is listed explicitly.

## CELT

| C file (reference `celt/`) | Rust module (`crates/opus-core/src/`) |
| --- | --- |
| `entcode.c`, `entenc.c`, `entdec.c`, `laplace.c` | `celt/range_coder.rs` |
| `cwrs.c` | `celt/cwrs.rs` |
| `bands.c` | `celt/bands.rs` |
| `celt_decoder.c` | `celt/decoder.rs` |
| `celt_encoder.c` | `celt/encoder.rs` |
| `kiss_fft.c` | `celt/fft.rs` |
| `mathops.c` | `celt/math_ops.rs` |
| `mdct.c` | `celt/mdct.rs` |
| `modes.c`, `static_modes_*` | `celt/modes.rs` |
| `pitch.c` | `celt/pitch.rs` |
| `quant_bands.c` | `celt/quant_bands.rs` |
| `rate.c` | `celt/rate.rs` |
| `vq.c` | `celt/vq.rs` |
| `celt_lpc.c` | `celt/lpc.rs` |
| `arch.h` / SIMD dispatch | `celt/simd.rs` |

## SILK

| C files (reference `silk/`) | Rust module (`crates/opus-core/src/silk/`) |
| --- | --- |
| `CNG.c`, `PLC.c` | `decoder.rs` (CNG and PLC sections) |
| `decode_core.c`, `decode_frame.c`, `decode_indices.c`, `decode_parameters.c`, `decode_pulses.c`, `decoder_set_fs.c`, `dec_API.c` | `decoder.rs` |
| `NLSF2A.c`, `NLSF_decode.c`, `NLSF_stabilize.c`, `NLSF_unpack.c` | `decoder.rs` |
| `LPC_analysis_filter.c`, `LPC_inv_pred_gain.c`, `LPC_fit.c`, `ana_filt_bank_1.c`, `bwexpander.c`, `bwexpander_32.c` | `common.rs` (decoder-used helpers) |
| `LTP_analysis_filter_FIX.c`, `LTP_scale_ctrl_FIX.c` | `decoder.rs` |
| `stereo_decode_pred.c`, `stereo_MS_to_LR.c`, `stereo_LR_to_MS.c` | `decoder.rs` |
| `resampler.c`, `resampler_rom.c`, `resampler_private.h` | `decoder.rs` |
| `tables_*.c`, `tables_other.c`, `tables_gain.c`, `tables_LTP.c`, `tables_NLSF.c`, `tables_pitch_lag.c`, `tables_pulses_per_block.c` | `tables.rs` |
| Encoder-only: `encode_*.c`, `NSQ*.c`, `VQ*.c`, `VAD.c`, `HP_variable_cutoff.c`, `sum_sqr_shift.c`, etc. | `encoder.rs` (kept for core completeness, not exposed by the decoder API) |

## Top-level Opus

| C file (reference `src/`) | Rust module (`crates/opus-core/src/`) |
| --- | --- |
| `opus_decoder.c` | `opus/decoder.rs` |
| `opus_encoder.c` | `opus/encoder.rs` (not exposed by this decoder-focused change) |
| `opus.c` (packet parsing helpers) | `opus/mod.rs` and `opus/decoder.rs` packet sections |
| `opus_multistream*.c` | `opus/multistream.rs` (not exposed) |
| `repacketizer.c` | `opus/repacketizer.rs` |
| `mlp.c`, `dnn/*`, `dred_*` | **removed / disabled in this repository** |

## Public wrapper crates

- `crates/opus-decoder/src/decoder.rs` — typed public `Decoder`, per-module errors.
- `crates/opus-decoder/src/pcm.rs` — s16/s24/f32 conversion matching `opus_demo`.
- `crates/opus-decoder-cli/src/main.rs` — `ropusdec` CLI.
