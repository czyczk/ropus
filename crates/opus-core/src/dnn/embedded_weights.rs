//! Compile-time default DNN weight blob.
//!
//! This repository deliberately ships no ML weights. The `ml` cargo
//! feature keeps the module API compilable for upstream comparison, but
//! no embedded blob is produced.

/// Embedded weight blob: always empty.
pub const WEIGHTS_BLOB: &[u8] = &[];

/// Convenience: `true` iff `WEIGHTS_BLOB` has at least one record.
/// Used by `OpusDecoder::new` to decide whether to auto-load weights.
#[inline]
pub const fn has_embedded_weights() -> bool {
    !WEIGHTS_BLOB.is_empty()
}
