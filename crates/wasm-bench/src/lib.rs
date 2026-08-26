//! Minimal no-WASI decode benchmark module for Node `WebAssembly`.

use std::panic::catch_unwind;
use std::slice::{from_raw_parts, from_raw_parts_mut};

use opus_decoder::{Channels, DecodeMode, Decoder};

/// Allocate `n` bytes and leak them for the JS side.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(n: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(n);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// JS must allocate at least this many bytes for the f32 output buffer.
pub const OUT_BUF_BYTES: usize = 64 * 1024 * 1024;

/// Decode an in-memory `opus_demo` raw stream (u32be len, u32be final range,
/// payload) to interleaved f32 at 48 kHz.
///
/// `channels` is 1 or 2. Returns samples per channel decoded, or a negative
/// code on error.
#[unsafe(no_mangle)]
pub extern "C" fn bench_decode(
    data_ptr: *const u8,
    data_len: usize,
    channels: u32,
    out_ptr: *mut u8,
) -> i64 {
    let result: Result<i64, i64> = catch_unwind(|| -> Result<i64, i64> {
        let data = unsafe { from_raw_parts(data_ptr, data_len) };
        let channels = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(-2),
        };
        let mut decoder = Decoder::new(48000, channels).map_err(|_| -3)?;
        let nch = channels.count();
        let out = unsafe { from_raw_parts_mut(out_ptr.cast::<f32>(), OUT_BUF_BYTES / 4) };

        let mut off = 0usize;
        let mut total_samples = 0usize;
        let mut out_off = 0usize;
        while off + 8 <= data.len() {
            let len = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            off += 8;
            if off + len > data.len() {
                return Err(-4);
            }
            let payload = &data[off..off + len];
            off += len;
            let n = decoder
                .decode_float(Some(payload), &mut out[out_off..], DecodeMode::Normal)
                .map_err(|_| -5)?;
            out_off += n * nch;
            total_samples += n;
        }
        if total_samples == 0 {
            return Err(-6);
        }
        Ok(total_samples as i64)
    })
    .unwrap_or(Err(-7));
    result.unwrap_or_else(|e| e)
}
