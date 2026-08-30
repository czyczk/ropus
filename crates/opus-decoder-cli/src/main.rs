//! `ropusdec` — single-stream Opus decoder CLI.
//!
//! Two input forms are supported:
//! - Ogg Opus (RFC 7845) files: channels/gain/pre-skip come from the
//!   `OpusHead` packet, matching the original `opusdec` input.
//! - `opus_demo` raw bitstreams (`u32be` packet length, `u32be` final range,
//!   payload) for the self-contained differential corpus.
//!
//! Output is a playable RIFF/WAVE file or headerless little-endian raw PCM in
//! `s16`, packed `s24`, or IEEE-float `f32`.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, ValueEnum};
use ogg::reading::PacketReader;
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
    about = "Decode a single-stream Opus file (Ogg Opus or opus_demo raw stream) to WAV or raw PCM"
)]
struct Args {
    /// Input file: Ogg Opus (.opus) or opus_demo raw bitstream.
    input: PathBuf,

    /// Output file (.wav or raw, depending on --output-type).
    output: PathBuf,

    /// Decoder output sample rate in Hz (Opus API rate).
    #[arg(long, default_value_t = 48000)]
    rate: u32,

    /// Stream channel count. Required for opus_demo raw streams; for Ogg input
    /// it is optional and must match the OpusHead header when supplied.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=2))]
    channels: Option<u8>,

    /// Output container.
    #[arg(long, value_enum, default_value_t = OutputType::Wav)]
    output_type: OutputType,

    /// Output sample encoding.
    #[arg(long, value_enum, default_value_t = SampleFormat::S16)]
    sample_format: SampleFormat,
}

const MAX_FRAME_SAMPLES_PER_CHANNEL: usize = 5760; // 120 ms at 48 kHz
const OGG_CAPTURE: &[u8; 4] = b"OggS";

fn main() -> Result<()> {
    let args = Args::parse();

    let mut file = File::open(&args.input)
        .with_context(|| format!("failed to open input {}", args.input.display()))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).unwrap_or_default();
    file.seek(SeekFrom::Start(0))?;

    let decoded = if &magic == OGG_CAPTURE {
        decode_ogg(&args, file)?
    } else {
        decode_raw_demo(&args, file)?
    };

    if decoded.bytes.is_empty() {
        return Err(anyhow!("no audio decoded from {}", args.input.display()));
    }

    let out = File::create(&args.output)
        .with_context(|| format!("failed to create output {}", args.output.display()))?;
    let mut out = BufWriter::new(out);
    let channels = decoded.channels as usize;
    match args.output_type {
        OutputType::Raw => {
            out.write_all(&decoded.bytes)
                .with_context(|| format!("failed writing raw PCM to {}", args.output.display()))?;
        }
        OutputType::Wav => {
            write_wav(
                &mut out,
                decoded.rate,
                channels,
                args.sample_format,
                &decoded.bytes,
            )
            .context("failed writing WAV output")?;
        }
    }
    out.flush().context("failed flushing output")?;
    eprintln!(
        "decoded {} PCM bytes ({} samples/channel) to {}",
        decoded.bytes.len(),
        decoded.bytes.len() / bytes_per_sample(args.sample_format) / channels,
        args.output.display()
    );
    Ok(())
}

struct DecodedAudio {
    bytes: Vec<u8>,
    rate: u32,
    channels: u8,
}

fn bytes_per_sample(format: SampleFormat) -> usize {
    match format {
        SampleFormat::S16 => 2,
        SampleFormat::S24 => 3,
        SampleFormat::F32 => 4,
    }
}

/// Reserve a rough initial output capacity from the encoded stream size.
///
/// Output/input byte ratios vary with bitrate (f32 mono at 48 kHz can exceed
/// 200:1 at the 6 kb/s minimum), so this is intentionally a floor, not an
/// exact bound. The cap keeps pathological overallocation bounded.
fn initial_output_capacity(format: SampleFormat, input_bytes: u64) -> usize {
    let ratio = match format {
        SampleFormat::S16 => 96,
        SampleFormat::S24 => 144,
        SampleFormat::F32 => 192,
    };
    usize::try_from(input_bytes.saturating_mul(ratio))
        .unwrap_or(usize::MAX)
        .min(64 * 1024 * 1024)
}

fn decode_raw_demo(args: &Args, mut input: File) -> Result<DecodedAudio> {
    let channels = match args.channels {
        Some(1) => Channels::Mono,
        Some(2) => Channels::Stereo,
        Some(n) => return Err(anyhow!("unsupported channel count {n}; expected 1 or 2")),
        None => {
            return Err(anyhow!(
                "--channels is required for opus_demo raw bitstream input"
            ));
        }
    };
    let mut decoder = new_decoder(args, channels)?;
    let mut decoded = Vec::<u8>::new();
    if let Ok(meta) = input.metadata() {
        decoded.reserve(initial_output_capacity(args.sample_format, meta.len()));
    }
    let mut header = [0u8; 8];
    let mut payload = Vec::new();
    let mut packet_index = 0u64;
    let interleaved = MAX_FRAME_SAMPLES_PER_CHANNEL * channels.count();
    let mut pcm_i32 = vec![0i32; interleaved];
    let mut pcm_f32 = vec![0f32; interleaved];

    loop {
        match input.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(anyhow!(
                    "failed reading packet header from {}: {e}",
                    args.input.display()
                ));
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
        decode_packet(
            args.sample_format,
            &mut decoder,
            &payload,
            channels,
            &mut decoded,
            &mut pcm_i32,
            &mut pcm_f32,
        )
        .with_context(|| format!("decoding packet {packet_index} failed"))?;
    }

    if packet_index == 0 {
        return Err(anyhow!(
            "input {} contained no Opus packets",
            args.input.display()
        ));
    }

    Ok(DecodedAudio {
        bytes: decoded,
        rate: args.rate,
        channels: channels.count() as u8,
    })
}

fn decode_ogg(args: &Args, file: File) -> Result<DecodedAudio> {
    let initial_capacity = file
        .metadata()
        .map(|m| initial_output_capacity(args.sample_format, m.len()))
        .unwrap_or(0);
    if args.rate != 48000 {
        return Err(anyhow!(
            "Ogg input currently requires --rate 48000 (resampling is not implemented yet)"
        ));
    }
    let mut reader: PacketReader<Box<dyn ReadSeek>> =
        PacketReader::new(Box::new(BufReader::new(file)));

    let head_pkt = reader
        .read_packet()
        .context("failed reading Ogg pages")?
        .ok_or_else(|| anyhow!("no packets found in Ogg input"))?;
    let head = parse_opus_head(&head_pkt.data)?;

    if let Some(requested) = args.channels
        && requested != head.channels
    {
        return Err(anyhow!(
            "--channels {requested} does not match OpusHead channels {}",
            head.channels
        ));
    }
    let channels = match head.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        n => {
            return Err(anyhow!(
                "unsupported OpusHead channel count {n}; expected 1 or 2"
            ));
        }
    };

    // OpusTags packet is required by RFC 7845 but does not affect PCM output.
    let tags = reader
        .read_packet()
        .context("failed reading Ogg packets")?
        .ok_or_else(|| anyhow!("Ogg input ended after OpusHead"))?;
    if tags.data.len() < 8 || &tags.data[..8] != b"OpusTags" {
        return Err(anyhow!("second Ogg packet is not OpusTags"));
    }

    let mut decoder = new_decoder(args, channels)?;
    if head.output_gain != 0 {
        decoder
            .set_gain(i32::from(head.output_gain))
            .context("applying OpusHead output gain")?;
    }

    let mut decoded = Vec::<u8>::with_capacity(initial_capacity);
    let mut packet_index = 0u64;
    let mut last_granule = 0u64;
    let interleaved = MAX_FRAME_SAMPLES_PER_CHANNEL * channels.count();
    let mut pcm_i32 = vec![0i32; interleaved];
    let mut pcm_f32 = vec![0f32; interleaved];
    loop {
        let packet = reader.read_packet().context("failed reading Ogg packets")?;
        let Some(packet) = packet else { break };
        packet_index += 1;
        last_granule = packet.absgp_page();
        decode_packet(
            args.sample_format,
            &mut decoder,
            &packet.data,
            channels,
            &mut decoded,
            &mut pcm_i32,
            &mut pcm_f32,
        )
        .with_context(|| format!("decoding Ogg packet {packet_index} failed"))?;
    }

    if packet_index == 0 {
        return Err(anyhow!("Ogg input contained no Opus audio packets"));
    }

    // OpusHead.pre_skip samples per channel must be trimmed at the 48 kHz
    // codec rate (RFC 7845 section 4.2); the last page granule position then
    // determines the exact end of the stream, so truncate any encoder
    // padding after that point.
    let bytes_per_ch = bytes_per_sample(args.sample_format);
    let skip_bytes = usize::from(head.pre_skip) * usize::from(head.channels) * bytes_per_ch;
    if skip_bytes > decoded.len() {
        return Err(anyhow!(
            "OpusHead pre_skip {} trims more samples than decoded",
            head.pre_skip
        ));
    }
    decoded.drain(..skip_bytes);

    if last_granule > u64::from(head.pre_skip) {
        let desired_samples = (last_granule - u64::from(head.pre_skip)) * u64::from(head.channels);
        let desired_bytes = usize::try_from(desired_samples)
            .ok()
            .and_then(|s| s.checked_mul(bytes_per_ch))
            .ok_or_else(|| anyhow!("Ogg granule-derived output size overflows"))?;
        if desired_bytes < decoded.len() {
            decoded.truncate(desired_bytes);
        }
    }

    Ok(DecodedAudio {
        bytes: decoded,
        rate: args.rate,
        channels: channels.count() as u8,
    })
}

fn new_decoder(args: &Args, channels: Channels) -> Result<Decoder> {
    Decoder::new(args.rate, channels).with_context(|| {
        format!(
            "failed to create decoder for {} Hz, {} channel(s)",
            args.rate,
            channels.count()
        )
    })
}

fn decode_packet(
    format: SampleFormat,
    decoder: &mut Decoder,
    payload: &[u8],
    channels: Channels,
    out: &mut Vec<u8>,
    pcm_i32: &mut [i32],
    pcm_f32: &mut [f32],
) -> Result<()> {
    match format {
        SampleFormat::S16 => {
            // Match `opus_demo -16`, which decodes through the 24-bit path and
            // rounds/saturates back to s16 (this clamps -2^23 to -32767).
            let n = decoder.decode24(Some(payload), pcm_i32, DecodeMode::Normal)?;
            out.extend(
                pcm_i32[..n * channels.count()]
                    .iter()
                    .map(|s| pcm::i24_to_s16(*s))
                    .flat_map(|s| s.to_le_bytes()),
            );
        }
        SampleFormat::S24 => {
            let n = decoder.decode24(Some(payload), pcm_i32, DecodeMode::Normal)?;
            out.extend(
                pcm_i32[..n * channels.count()]
                    .iter()
                    .map(|s| pcm::i24_to_s24(*s))
                    .flat_map(|s| {
                        let b = s.to_le_bytes();
                        [b[0], b[1], b[2]]
                    }),
            );
        }
        SampleFormat::F32 => {
            let n = decoder.decode_float(Some(payload), pcm_f32, DecodeMode::Normal)?;
            out.extend(
                pcm_f32[..n * channels.count()]
                    .iter()
                    .flat_map(|s| s.to_le_bytes()),
            );
        }
    }
    Ok(())
}

struct OpusHead {
    channels: u8,
    pre_skip: u16,
    output_gain: i16,
}

fn parse_opus_head(data: &[u8]) -> Result<OpusHead> {
    if data.len() != 19 || &data[..8] != b"OpusHead" {
        return Err(anyhow!("malformed OpusHead packet"));
    }
    let version = data[8];
    if version == 0 || (version & 0xF0) != 0 {
        return Err(anyhow!("unsupported OpusHead version 0x{version:02x}"));
    }
    let channels = data[9];
    let pre_skip = u16::from_le_bytes(data[10..12].try_into().expect("2 bytes"));
    let input_sample_rate = u32::from_le_bytes(data[12..16].try_into().expect("4 bytes"));
    if input_sample_rate != 48000 {
        return Err(anyhow!(
            "unsupported OpusHead input sample rate {input_sample_rate}; expected 48000"
        ));
    }
    let output_gain = i16::from_le_bytes(data[16..18].try_into().expect("2 bytes"));
    if data[18] != 0 {
        return Err(anyhow!(
            "unsupported OpusHead channel mapping family {}; expected family 0",
            data[18]
        ));
    }
    Ok(OpusHead {
        channels,
        pre_skip,
        output_gain,
    })
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

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
