//! `ropusdec` — single-stream Opus decoder CLI.
//!
//! Input is the raw packet stream written by the reference `opus_demo`
//! encoder (`u32be` packet length, `u32be` final range, payload). Output is a
//! playable RIFF/WAVE file or headerless little-endian raw PCM in `s16`,
//! packed `s24`, or IEEE-float `f32`.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use opus_decoder::pcm::{self, Error as PcmError};
use opus_decoder::{Channels, DecodeMode, Decoder};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputType {
    /// RIFF/WAVE container.
    Wav,
    /// Headerless little-endian PCM.
    Raw,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SampleFormat {
    /// Signed 16-bit PCM.
    S16,
    /// Packed signed 24-bit PCM.
    S24,
    /// IEEE-754 32-bit float PCM.
    F32,
}

#[derive(Debug, Parser)]
#[command(
    name = "ropusdec",
    version,
    about = "Decode a single-stream Opus packet file (opus_demo raw stream) to WAV or raw PCM"
)]
struct Args {
    /// Input file: opus_demo raw bitstream (u32be len, u32be final_range, payload).
    input: PathBuf,

    /// Output file (.wav or raw, depending on --output-type).
    output: PathBuf,

    /// Decoder output sample rate in Hz.
    #[arg(long, default_value_t = 48000)]
    rate: u32,

    /// Stream channel count (opus_demo raw streams do not carry a container header).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=2))]
    channels: u8,

    /// Output container.
    #[arg(long, value_enum, default_value_t = OutputType::Wav)]
    output_type: OutputType,

    /// Output sample encoding.
    #[arg(long, value_enum, default_value_t = SampleFormat::S16)]
    sample_format: SampleFormat,
}

const MAX_FRAME_SAMPLES_PER_CHANNEL: usize = 5760; // 120 ms at 48 kHz

fn main() -> Result<()> {
    let args = Args::parse();

    let channels = match args.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        n => return Err(anyhow!("unsupported channel count {n}; expected 1 or 2")),
    };

    let mut decoder = Decoder::new(args.rate, channels).with_context(|| {
        format!(
            "failed to create decoder for {} Hz, {} channel(s)",
            args.rate,
            channels.count()
        )
    })?;

    let mut input = BufReader::new(
        File::open(&args.input)
            .with_context(|| format!("failed to open input {}", args.input.display()))?,
    );

    let mut decoded = Vec::<u8>::new();
    let mut header = [0u8; 8];
    let mut payload = Vec::new();
    let mut packet_index = 0u64;

    loop {
        match input.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(anyhow!(
                    "failed reading packet header from {}: {e}",
                    args.input.display()
                ))
                .context("I/O error while reading input")
            }
        }
        let len = u32::from_be_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
        payload.resize(len, 0);
        input.read_exact(&mut payload).with_context(|| {
            format!(
                "truncated packet payload in {} at packet {packet_index}",
                args.input.display()
            )
        })?;
        packet_index += 1;

        let interleaved = MAX_FRAME_SAMPLES_PER_CHANNEL * channels.count();
        match args.sample_format {
            SampleFormat::S16 => {
                let mut pcm = vec![0i16; interleaved];
                let n = decoder
                    .decode(Some(&payload), &mut pcm, DecodeMode::Normal)
                    .with_context(|| format!("decoding packet {packet_index} failed"))?;
                for s in &pcm[..n * channels.count()] {
                    decoded.extend_from_slice(&s.to_le_bytes());
                }
            }
            SampleFormat::S24 => {
                let mut pcm = vec![0i32; interleaved];
                let n = decoder
                    .decode24(Some(&payload), &mut pcm, DecodeMode::Normal)
                    .with_context(|| format!("decoding packet {packet_index} failed"))?;
                for s in &pcm[..n * channels.count()] {
                    let s = pcm::i24_to_s24(*s);
                    decoded.extend_from_slice(&s.to_le_bytes()[..3]);
                }
            }
            SampleFormat::F32 => {
                let mut pcm = vec![0f32; interleaved];
                let n = decoder
                    .decode_float(Some(&payload), &mut pcm, DecodeMode::Normal)
                    .with_context(|| format!("decoding packet {packet_index} failed"))?;
                for s in &pcm[..n * channels.count()] {
                    decoded.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
    }

    if packet_index == 0 {
        return Err(anyhow!(
            "input {} contained no Opus packets",
            args.input.display()
        ));
    }

    let out = File::create(&args.output)
        .with_context(|| format!("failed to create output {}", args.output.display()))?;
    let mut out = BufWriter::new(out);

    match args.output_type {
        OutputType::Raw => {
            out.write_all(&decoded)
                .with_context(|| format!("failed writing raw PCM to {}", args.output.display()))?;
        }
        OutputType::Wav => {
            write_wav(
                &mut out,
                args.rate,
                channels.count(),
                args.sample_format,
                &decoded,
            )
            .context("failed writing WAV output")?;
        }
    }

    out.flush().context("failed flushing output")?;
    eprintln!(
        "decoded {packet_index} packet(s), {} PCM bytes to {}",
        decoded.len(),
        args.output.display()
    );
    Ok(())
}

fn write_wav(
    out: &mut impl Write,
    rate: u32,
    channels: usize,
    format: SampleFormat,
    data: &[u8],
) -> Result<()> {
    pcm::validate_channels(channels).map_err(PcmError::into_anyhow)?;

    let (format_tag, bits_per_sample, block_align): (u16, u16, u16) = match format {
        SampleFormat::S16 => (1, 16, (channels * 2) as u16),
        SampleFormat::S24 => (1, 24, (channels * 3) as u16),
        SampleFormat::F32 => (3, 32, (channels * 4) as u16),
    };
    let byte_rate = rate
        .checked_mul(block_align as u32)
        .ok_or_else(|| anyhow!("WAV byte rate overflow"))?;
    let data_len = u32::try_from(data.len()).context("WAV data exceeds 4 GiB")?;
    let riff_len = data_len
        .checked_add(36)
        .ok_or_else(|| anyhow!("WAV size overflow"))?;

    out.write_all(b"RIFF")?;
    out.write_all(&riff_len.to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&format_tag.to_le_bytes())?;
    out.write_all(&(channels as u16).to_le_bytes())?;
    out.write_all(&rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&bits_per_sample.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    out.write_all(data)?;
    Ok(())
}

/// Small extension trait to keep `PcmError` conversions out of the main flow.
trait IntoAnyhow {
    fn into_anyhow(self) -> anyhow::Error;
}

impl IntoAnyhow for PcmError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::new(self)
    }
}
