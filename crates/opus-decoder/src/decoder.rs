//! Public single-stream decoder.
//!
//! Wraps the low-level, C-isomorphic [`OpusDecoder`] from `opus-core` with a
//! typed, per-module error surface.

use opus_core::opus::decoder::OpusDecoder as CoreDecoder;
use thiserror::Error;

/// Errors owned by the public decoder module.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// The requested output sample rate is not an Opus API rate.
    #[error("unsupported sample rate {0}; expected 8000, 12000, 16000, 24000, or 48000 Hz")]
    UnsupportedSampleRate(u32),
    /// The requested channel count is not supported for a single stream.
    #[error("unsupported channel count {0}; expected 1 or 2")]
    UnsupportedChannels(u32),
    /// The packet or a decoder argument is invalid.
    #[error("bad argument")]
    BadArgument,
    /// The provided output buffer is too small.
    #[error("output buffer too small")]
    BufferTooSmall,
    /// The packet cannot be decoded (malformed TOC, truncated payload, or
    /// corrupt entropy-coded data).
    #[error("invalid packet")]
    InvalidPacket,
    /// The codec core hit an internal invariant failure.
    #[error("internal decoder error")]
    Internal,
    /// The requested operation is valid but not implemented.
    #[error("operation not implemented")]
    Unimplemented,
}

fn from_core_code(code: i32) -> Error {
    use opus_core::opus::decoder::{
        OPUS_BAD_ARG, OPUS_BUFFER_TOO_SMALL, OPUS_INTERNAL_ERROR, OPUS_INVALID_PACKET,
        OPUS_UNIMPLEMENTED,
    };
    match code {
        OPUS_BAD_ARG => Error::BadArgument,
        OPUS_BUFFER_TOO_SMALL => Error::BufferTooSmall,
        OPUS_INVALID_PACKET => Error::InvalidPacket,
        OPUS_INTERNAL_ERROR => Error::Internal,
        OPUS_UNIMPLEMENTED => Error::Unimplemented,
        _ => Error::Internal,
    }
}

/// Number of channels in a decoded stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    /// One channel.
    Mono,
    /// Two interleaved channels.
    Stereo,
}

impl Channels {
    /// Number of interleaved channels.
    pub fn count(self) -> usize {
        match self {
            Channels::Mono => 1,
            Channels::Stereo => 2,
        }
    }
}

/// Decoding mode for a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeMode {
    /// Normal decode of the packet payload.
    Normal,
    /// Decode in-band FEC data (recovers the previous frame).
    Fec,
}

impl DecodeMode {
    fn as_decode_fec(self) -> bool {
        match self {
            DecodeMode::Normal => false,
            DecodeMode::Fec => true,
        }
    }
}

/// Stateful single-stream Opus decoder.
pub struct Decoder {
    inner: CoreDecoder,
    sample_rate: u32,
    channels: Channels,
}

impl Decoder {
    /// Create a decoder for the given output sample rate and channel count.
    ///
    /// Valid rates are 8000, 12000, 16000, 24000, and 48000 Hz; valid channel
    /// counts are 1 and 2 (matching `opus_decoder_create`).
    pub fn new(sample_rate: u32, channels: Channels) -> Result<Self, Error> {
        if !matches!(sample_rate, 8000 | 12000 | 16000 | 24000 | 48000) {
            return Err(Error::UnsupportedSampleRate(sample_rate));
        }
        let core = CoreDecoder::new(sample_rate as i32, channels.count() as i32)
            .map_err(from_core_code)?;
        Ok(Self {
            inner: core,
            sample_rate,
            channels,
        })
    }

    /// Decode one packet to interleaved `s16` PCM.
    ///
    /// `packet` is the Opus packet payload, or `None` to run packet loss
    /// concealment for one missing frame. Returns samples per channel written.
    pub fn decode(
        &mut self,
        packet: Option<&[u8]>,
        output: &mut [i16],
        mode: DecodeMode,
    ) -> Result<usize, Error> {
        let frame_size = self.frame_size_from_output(output.len())?;
        let n = self
            .inner
            .decode(packet, output, frame_size, mode.as_decode_fec())
            .map_err(from_core_code)?;
        Ok(n as usize)
    }

    /// Decode one packet to interleaved 24-bit PCM (`i32`, sign-extended).
    pub fn decode24(
        &mut self,
        packet: Option<&[u8]>,
        output: &mut [i32],
        mode: DecodeMode,
    ) -> Result<usize, Error> {
        let frame_size = self.frame_size_from_output(output.len())?;
        let n = self
            .inner
            .decode24(packet, output, frame_size, mode.as_decode_fec())
            .map_err(from_core_code)?;
        Ok(n as usize)
    }

    /// Decode one packet to interleaved `f32` PCM in the nominal [-1, 1)
    /// range used by the reference fixed-point decoder.
    pub fn decode_float(
        &mut self,
        packet: Option<&[u8]>,
        output: &mut [f32],
        mode: DecodeMode,
    ) -> Result<usize, Error> {
        let frame_size = self.frame_size_from_output(output.len())?;
        let n = self
            .inner
            .decode_float(packet, output, frame_size, mode.as_decode_fec())
            .map_err(from_core_code)?;
        Ok(n as usize)
    }

    fn frame_size_from_output(&self, interleaved_len: usize) -> Result<i32, Error> {
        let nch = self.channels.count();
        if nch == 0 || interleaved_len % nch != 0 {
            return Err(Error::BadArgument);
        }
        let per_channel = interleaved_len / nch;
        i32::try_from(per_channel).map_err(|_| Error::BadArgument)
    }

    /// Reset decoder state, preserving no stream history.
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Configured output sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Configured channel layout.
    pub fn channels(&self) -> Channels {
        self.channels
    }

    /// Final range value of the most recently decoded packet (diagnostic).
    pub fn final_range(&self) -> u32 {
        self.inner.get_final_range()
    }

    /// Duration of the most recently decoded packet in samples per channel.
    pub fn last_packet_duration(&self) -> usize {
        self.inner.get_last_packet_duration().max(0) as usize
    }

    /// Number of samples per channel the next decode call can produce for a
    /// known packet payload.
    pub fn nb_samples(&self, packet: &[u8]) -> Result<usize, Error> {
        self.inner
            .get_nb_samples(packet)
            .map(|n| n.max(0) as usize)
            .map_err(from_core_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_configuration() {
        assert!(matches!(
            Decoder::new(44100, Channels::Mono),
            Err(Error::UnsupportedSampleRate(44100))
        ));
        assert!(Decoder::new(48000, Channels::Mono).is_ok());
        assert!(Decoder::new(8000, Channels::Stereo).is_ok());
    }

    #[test]
    fn buffer_length_must_match_channel_count() {
        let mut dec = Decoder::new(48000, Channels::Stereo).unwrap();
        let mut odd = [0i16; 3];
        assert_eq!(dec.decode(None, &mut odd, DecodeMode::Normal), Err(Error::BadArgument));
    }

    #[test]
    fn missing_packet_uses_plc() {
        let mut dec = Decoder::new(48000, Channels::Mono).unwrap();
        let mut out = [0i16; 960];
        let n = dec.decode(None, &mut out, DecodeMode::Normal).unwrap();
        assert_eq!(n, 960);
        assert!(out.iter().all(|&s| s == 0));
    }
}
