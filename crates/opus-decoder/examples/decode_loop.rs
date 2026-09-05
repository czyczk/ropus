//! Profiling driver: decode one corpus file in a loop forever.
//!
//! Usage: decode_loop <corpus-file> <channels>
//! Attach with `sample <pid> <seconds>` or a time profiler while it runs.

use opus_decoder::{Channels, DecodeMode, Decoder};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

fn read_packets(name: &str) -> Vec<Vec<u8>> {
    let path = PathBuf::from(name);
    let mut data = Vec::new();
    File::open(&path).unwrap().read_to_end(&mut data).unwrap();
    let mut packets = Vec::new();
    let mut off = 0usize;
    while off + 8 <= data.len() {
        let len = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        off += 8;
        packets.push(data[off..off + len].to_vec());
        off += len;
    }
    packets
}

fn main() {
    let file = std::env::args().nth(1).expect("corpus file");
    let channels = match std::env::args().nth(2).as_deref() {
        Some("2") => Channels::Stereo,
        _ => Channels::Mono,
    };
    let packets = read_packets(&file);
    let mut dec = Decoder::new(48000, channels).unwrap();
    let mut out = vec![0f32; 5760 * channels.count()];
    let mut acc = 0.0f64;
    loop {
        for p in &packets {
            let n = dec
                .decode_float(Some(p.as_slice()), &mut out, DecodeMode::Normal)
                .unwrap();
            acc += out[0] as f64;
            std::hint::black_box(&mut out[..n * channels.count()]);
        }
        dec = Decoder::new(48000, channels).unwrap();
        std::hint::black_box(acc);
    }
}
