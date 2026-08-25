//! PCM output conversion helpers.
//!
//! Conversion rules mirror `opus_demo`'s 24-bit decode path in libopus
//! v1.6.1: decode to a sign-extended 24-bit value in an `i32`, then either
//! round/saturate to `s16`, pack the low 24 bits as `s24`, or scale to `f32`.

use thiserror::Error;

/// Errors owned by the PCM conversion module.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Channel count is not supported for interleaved output.
    #[error("unsupported channel count {0}; expected 1 or 2")]
    InvalidChannelCount(usize),
    /// Output buffer is too small for the requested sample count.
    #[error("output buffer too small: needed {needed} interleaved samples, got {got}")]
    OutputTooSmall { needed: usize, got: usize },
}

const S16_MAX_I24: i32 = 0x007f_ff00;
const S16_MIN_I24: i32 = -0x007f_ff00;
const S24_MAX: i32 = 0x007f_ffff;
const S24_MIN: i32 = -0x007f_ffff;

/// Validate an interleaved channel count for PCM output.
pub fn validate_channels(channels: usize) -> Result<(), Error> {
    if channels == 1 || channels == 2 {
        Ok(())
    } else {
        Err(Error::InvalidChannelCount(channels))
    }
}

/// Convert a decoded 24-bit sample (`i32`, sign-extended) to `i16` exactly as
/// `opus_demo` does: clamp to the `s16` representable range in Q24 and round
/// with `(s + 128) >> 8` (arithmetic shift).
pub fn i24_to_s16(s: i32) -> i16 {
    let s = s.clamp(S16_MIN_I24, S16_MAX_I24);
    ((s + 128) >> 8) as i16
}

/// Convert a decoded 24-bit sample (`i32`, sign-extended) to a packed `s24`
/// value, saturating to the signed 24-bit range.
pub fn i24_to_s24(s: i32) -> i32 {
    s.clamp(S24_MIN, S24_MAX)
}

/// Scale a decoded 24-bit sample to `f32` exactly as `opus_demo -f32` does.
#[inline]
pub fn i24_to_f32(s: i32) -> f32 {
    s as f32 * (1.0 / 8_388_608.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_validated() {
        assert!(validate_channels(1).is_ok());
        assert!(validate_channels(2).is_ok());
        assert_eq!(validate_channels(0), Err(Error::InvalidChannelCount(0)));
        assert_eq!(validate_channels(3), Err(Error::InvalidChannelCount(3)));
    }

    #[test]
    fn s16_conversion_matches_opus_demo_rules() {
        assert_eq!(i24_to_s16(0), 0);
        assert_eq!(i24_to_s16(128), 1);
        assert_eq!(i24_to_s16(127), 0);
        assert_eq!(i24_to_s16(-128), -1);
        assert_eq!(i24_to_s16(S24_MAX), 32767);
        assert_eq!(i24_to_s16(S24_MIN), -32768);
        assert_eq!(i24_to_s16(i32::MAX), 32767);
        assert_eq!(i24_to_s16(i32::MIN), -32768);
    }

    #[test]
    fn s24_conversion_saturates() {
        assert_eq!(i24_to_s24(0), 0);
        assert_eq!(i24_to_s24(S24_MAX), S24_MAX);
        assert_eq!(i24_to_s24(S24_MIN), S24_MIN);
        assert_eq!(i24_to_s24(i32::MAX), S24_MAX);
        assert_eq!(i24_to_s24(i32::MIN), S24_MIN);
    }

    #[test]
    fn f32_scaling_uses_decode24_denominator() {
        assert_eq!(i24_to_f32(8_388_608), 1.0);
        assert_eq!(i24_to_f32(-8_388_608), -1.0);
        assert_eq!(i24_to_f32(0), 0.0);
    }
}
