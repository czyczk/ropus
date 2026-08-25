use opus_core::{Channels, DecodeMode, Decoder};
use std::env;
use std::fs::File;
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        eprintln!("usage: decode_dump <rate> <channels> <input.opus> <output-f32-le>");
        std::process::exit(2);
    }
    let rate: u32 = args[1].parse().unwrap();
    let channels = match args[2].parse::<u32>().unwrap() {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        _ => panic!("channels must be 1 or 2"),
    };
    let mut input = File::open(&args[3]).unwrap();
    let mut out = File::create(&args[4]).unwrap();
    let mut decoder = Decoder::new(rate, channels).unwrap();

    let mut header = [0u8; 8];
    let mut payload = Vec::new();
    let mut total = 0u64;
    loop {
        let n = input.read(&mut header).unwrap();
        if n == 0 {
            break;
        }
        if n != 8 {
            eprintln!("truncated header at {}", total);
            std::process::exit(3);
        }
        let len = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
        payload.resize(len, 0);
        if input.read_exact(&mut payload).is_err() {
            eprintln!("truncated payload");
            std::process::exit(3);
        }
        let max_samples = (rate / 1000 * 120) as usize;
        let mut pcm = vec![0f32; max_samples * args[2].parse::<usize>().unwrap()];
        let n_samples = decoder
            .decode_float(&payload, &mut pcm, DecodeMode::Normal)
            .unwrap();
        let bytes = n_samples * args[2].parse::<usize>().unwrap();
        let mut f32buf = vec![0u8; bytes * 4];
        for (i, s) in pcm[..bytes].iter().enumerate() {
            f32buf[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out.write_all(&f32buf).unwrap();
        total += bytes as u64;
    }
    eprintln!("decoded {} samples/channel total", total / args[2].parse::<u64>().unwrap());
}
