//! Single-stream Opus decoder library (RFC 6716: CELT + SILK).
//!
//! This crate is the public decoder surface of the Rust Opus port. The
//! algorithmic core lives in [`opus_core`]; module organization there mirrors
//! the C reference (`celt/*.c`, `silk/*.c`, `src/*.c`).

pub mod decoder;
pub mod pcm;

pub use decoder::{Channels, DecodeMode, Decoder};
