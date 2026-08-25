use criterion::{Criterion, black_box, criterion_group, criterion_main};
use opus_decoder::{Channels, DecodeMode, Decoder};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

fn read_packets(name: &str) -> Vec<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name);
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

fn decode_all(packets: &[Vec<u8>], channels: Channels) {
    let mut dec = Decoder::new(48000, channels).unwrap();
    let mut out = vec![0f32; 5760 * channels.count()];
    for p in packets {
        let n = dec
            .decode_float(
                black_box(Some(p.as_slice())),
                black_box(&mut out),
                DecodeMode::Normal,
            )
            .unwrap();
        black_box(&out[..n * channels.count()]);
    }
}

fn bench_case(c: &mut Criterion, id: &str, file: &str, channels: Channels) {
    let packets = read_packets(file);
    c.bench_function(id, |b| b.iter(|| decode_all(&packets, channels)));
}

fn benches(c: &mut Criterion) {
    bench_case(
        c,
        "decode/celt-stereo-96k",
        "music-a-celt-096k-20ms.opus",
        Channels::Stereo,
    );
    bench_case(
        c,
        "decode/silk-mono-12k",
        "speech-silk-012k-20ms.opus",
        Channels::Mono,
    );
    bench_case(
        c,
        "decode/hybrid-mono-32k",
        "speech-hybrid-032k-20ms.opus",
        Channels::Mono,
    );
    bench_case(
        c,
        "decode/silk-dtx-mono-12k",
        "speech-silk-012k-dtx-20ms.opus",
        Channels::Mono,
    );
}

criterion_group!(name = decode_benches; config = Criterion::default().sample_size(20); targets = benches);
criterion_main!(decode_benches);
