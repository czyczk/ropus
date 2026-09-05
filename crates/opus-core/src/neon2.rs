//! Explicit aarch64 NEON kernels ("second NEON tier").
//!
//! These kernels cover hot loops that neither LLVM auto-vectorisation nor the
//! portable `wide` kernels handle well on aarch64 (M1 Pro `sample` evidence in
//! PERFORMANCE.md):
//!
//! - SILK resampler: 8-tap fractional FIR interpolation (`iir_fir`) and the
//!   2x high-quality allpass upsampler (`up2_hq`) — together ~32% of SILK
//!   decode self time.
//! - SILK recursive LPC/LTP synthesis dot products (moved here from
//!   `silk::decoder`), now with per-subframe pre-reversed coefficient vectors
//!   so the state window loads ascending with no per-sample `vrev64`/`vext`
//!   shuffles, and with four independent accumulator chains.
//! - CELT MDCT dynamic-range scan (`maxval`/`sumval`).
//!
//! Rejected after kernel-level A/B (see PERFORMANCE.md): stereo deemphasis
//! (two-lane packing of a latency-bound IIR chain was ~10% slower than the
//! interleaved scalar pair).
//!
//! Every kernel is bit-exact against its scalar counterpart: per-lane products
//! and shifts are identical to the scalar ops, and wrapping integer addition
//! is associative modulo 2^32, so reassociating the accumulation order is
//! exact. All kernels are gated on `all(target_arch = "aarch64", feature =
//! "neon2")` at the call site.

use std::arch::aarch64::*;

use crate::silk::tables::MAX_LPC_ORDER;

// ===========================================================================
// SILK: LTP prediction dot product (5 taps)
// ===========================================================================

/// NEON dot-product for one voiced SILK LTP prediction sample.
///
/// Computes `Σ_{j=0..5} (state[p-j] * b_q14[j]) >> 16` summed in i64 and
/// truncated to i32 (identical modulo 2^32 to the scalar wrapping chain).
///
/// `b_rev` is the reversed 5-tap coefficient vector (`b_rev[k] = b_q14[4-k]`)
/// prepared once per subframe, so the state window `state[p-4..=p]` loads
/// ascending with no shuffles. Reads `state[p-4..=p]`, same as scalar.
#[inline(always)]
pub(crate) fn silk_ltp_pred_neon(state: &[i32], b_rev: &[i16; 8], pred_base: i64) -> i32 {
    unsafe {
        let p = pred_base as usize;
        // Lanes k=0..3: state[p-4+k] * b_rev[k] = state[p-4+k] * b_q14[4-k].
        let sv = vld1q_s32(state.as_ptr().add(p - 4));
        let cv = vmovl_s16(vld1_s16(b_rev.as_ptr()));
        let lo = vshrq_n_s64(vmull_s32(vget_low_s32(sv), vget_low_s32(cv)), 16);
        let hi = vshrq_n_s64(vmull_s32(vget_high_s32(sv), vget_high_s32(cv)), 16);
        let mut sum = vgetq_lane_s64(lo, 0)
            + vgetq_lane_s64(lo, 1)
            + vgetq_lane_s64(hi, 0)
            + vgetq_lane_s64(hi, 1);
        // Scalar center tap: state[p] * b_q14[0].
        sum += (state[p] as i64 * i64::from(b_rev[4])) >> 16;
        sum as i32
    }
}


// ===========================================================================
// CELT: comb filter constant region (trailing-read FIR, in-place safe)
// ===========================================================================

/// NEON constant-parameter comb-filter region of `comb_filter`: per sample,
/// `val = x[i] + g10·x2 + g11·(x1+x3) + g12·(x0+x4)`, `y[i] = sat(val - 1)`.
///
/// The reads trail the writes by `t-2 >= 13` samples (`t >=
/// COMBFILTER_MINPERIOD = 15`), so processing four consecutive samples per
/// block never reads a value written by the same pass (in-place safe).
/// Bit-exact: `mult16_32_q15` per lane matches the scalar 64-bit recipe; the
/// four-term sum is wrapping i32 (order-free modulo 2^32).
pub(crate) fn comb_filter_const_neon(
    buf: &mut [i32],
    off: usize,
    count: usize,
    t1: usize,
    g10: i32,
    g11: i32,
    g12: i32,
) -> usize {
    use crate::types::SIG_SAT;
    debug_assert!(t1 >= 15);
    let g10v = unsafe { vdupq_n_s32(g10 as i16 as i32) };
    let g11v = unsafe { vdupq_n_s32(g11 as i16 as i32) };
    let g12v = unsafe { vdupq_n_s32(g12 as i16 as i32) };
    let sig_hi = unsafe { vdupq_n_s32(SIG_SAT) };
    let sig_lo = unsafe { vdupq_n_s32(-SIG_SAT) };
    let one = unsafe { vdupq_n_s32(1) };
    let vec_end = count & !3;
    let mut i = 0usize;
    while i < vec_end {
        let idx = off + i;
        unsafe {
            let x = vld1q_s32(buf.as_ptr().add(idx));
            let x2 = vld1q_s32(buf.as_ptr().add(idx - t1));
            let x1 = vld1q_s32(buf.as_ptr().add(idx + 1 - t1));
            let x3 = vld1q_s32(buf.as_ptr().add(idx - 1 - t1));
            let x0 = vld1q_s32(buf.as_ptr().add(idx + 2 - t1));
            let x4 = vld1q_s32(buf.as_ptr().add(idx - 2 - t1));
            let val = vaddq_s32(
                vaddq_s32(x, s_mul4(x2, g10v)),
                vaddq_s32(
                    s_mul4(vaddq_s32(x1, x3), g11v),
                    s_mul4(vaddq_s32(x0, x4), g12v),
                ),
            );
            let val = vminq_s32(vmaxq_s32(vsubq_s32(val, one), sig_lo), sig_hi);
            vst1q_s32(buf.as_mut_ptr().add(idx), val);
        }
        i += 4;
    }
    i
}

// ===========================================================================
// CELT: stereo merge band loop + norm inner products
// ===========================================================================

/// `mult32_32_q31(a, b) = ((a*b) >> 31) as i32` on four lanes (a splatted).
#[inline(always)]
fn mult32_32_q31_4(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        let lo = vshrq_n_s64(vmull_s32(vget_low_s32(a), vget_low_s32(b)), 31);
        let hi = vshrq_n_s64(vmull_high_s32(a, b), 31);
        vcombine_s32(vmovn_s64(lo), vmovn_s64(hi))
    }
}

/// NEON `celt_inner_prod_norm_shift`: i64 accumulate of `x[i]*y[i]`, then
/// `>> 20` (arithmetic) and truncate. Wrapping i64 accumulation is
/// order-independent modulo 2^64, so the lane split is exact.
pub(crate) fn celt_inner_prod_norm_shift_neon(x: &[i32], y: &[i32], n: usize) -> i32 {
    let mut acc0 = unsafe { vdupq_n_s64(0) };
    let mut acc1 = unsafe { vdupq_n_s64(0) };
    let chunks = n / 4;
    for c in 0..chunks {
        unsafe {
            let xv = vld1q_s32(x.as_ptr().add(c * 4));
            let yv = vld1q_s32(y.as_ptr().add(c * 4));
            acc0 = vaddq_s64(acc0, vmull_s32(vget_low_s32(xv), vget_low_s32(yv)));
            acc1 = vaddq_s64(acc1, vmull_high_s32(xv, yv));
        }
    }
    let mut sum = unsafe { vgetq_lane_s64(vaddq_s64(acc0, acc1), 0) + vgetq_lane_s64(vaddq_s64(acc0, acc1), 1) };
    for i in chunks * 4..n {
        sum = sum.wrapping_add(x[i] as i64 * y[i] as i64);
    }
    (sum >> (2 * (crate::types::NORM_SHIFT - 14))) as i32
}

/// NEON `stereo_merge` band loop:
/// `l = mult32_32_q31(mid, x[j])`, `x[j] = vshr32(mult32_32_q31(lgain, l-r), kl-15)`,
/// `y[j] = vshr32(mult32_32_q31(rgain, l+r), kr-15)`.
/// `vshr32` maps to one signed variable shift (negative amount = arithmetic
/// right shift, positive = wrapping left shift) per lane group.
pub(crate) fn stereo_merge_neon(
    x: &mut [i32],
    y: &mut [i32],
    mid: i32,
    lgain: i32,
    rgain: i32,
    kl: i32,
    kr: i32,
    n: usize,
) {
    let midv = unsafe { vdupq_n_s32(mid) };
    let lgainv = unsafe { vdupq_n_s32(lgain) };
    let rgainv = unsafe { vdupq_n_s32(rgain) };
    let shl = unsafe { vdupq_n_s32(-(kl - 15)) };
    let shr = unsafe { vdupq_n_s32(-(kr - 15)) };
    let chunks = n / 4;
    for c in 0..chunks {
        unsafe {
            let xv = vld1q_s32(x.as_ptr().add(c * 4));
            let yv = vld1q_s32(y.as_ptr().add(c * 4));
            let l = mult32_32_q31_4(midv, xv);
            let xl = vshlq_s32(mult32_32_q31_4(lgainv, vsubq_s32(l, yv)), shl);
            let yl = vshlq_s32(mult32_32_q31_4(rgainv, vaddq_s32(l, yv)), shr);
            vst1q_s32(x.as_mut_ptr().add(c * 4), xl);
            vst1q_s32(y.as_mut_ptr().add(c * 4), yl);
        }
    }
    let mut j = chunks * 4;
    while j < n {
        use crate::types::{mult32_32_q31, vshr32};
        let l = mult32_32_q31(mid, x[j]);
        let r = y[j];
        x[j] = vshr32(mult32_32_q31(lgain, l.wrapping_sub(r)), kl - 15);
        y[j] = vshr32(mult32_32_q31(rgain, l.wrapping_add(r)), kr - 15);
        j += 1;
    }
}

// ===========================================================================
// CELT: FFT butterflies (radix-3/4/5 used by the 480/240/120/60 states)
// ===========================================================================

/// Gather four twiddles at `base + k*step`, split into (r, i) lane vectors.
#[inline(always)]
unsafe fn tw4(
    tw: &[crate::celt::fft::KissTwiddleCpx],
    base: usize,
    step: usize,
) -> (int32x4_t, int32x4_t) {
    unsafe {
        // KissTwiddleCpx is repr(C) { i16 r, i16 i } → 4 bytes per twiddle,
        // but the struct only has 2-byte alignment, so a packed i32 read at
        // an odd element index would be misaligned; `read_unaligned` lowers
        // to a single unaligned-tolerant `ldr` on aarch64. Little-endian: r
        // is the low half.
        let p = tw.as_ptr() as *const i32;
        let packed = vld1q_s32(
            [
                p.add(base).read_unaligned(),
                p.add(base + step).read_unaligned(),
                p.add(base + 2 * step).read_unaligned(),
                p.add(base + 3 * step).read_unaligned(),
            ]
            .as_ptr(),
        );
        let r = vshrq_n_s32(vshlq_n_s32(packed, 16), 16);
        let i = vshrq_n_s32(packed, 16);
        (r, i)
    }
}

/// Complex multiply on four lanes: `(ar+i·ai) * (tr+i·ti)` with the scalar
/// per-component `s_mul` (>>15 truncate) recipe; adds wrap.
#[inline(always)]
fn cmul4(
    ar: int32x4_t,
    ai: int32x4_t,
    tr: int32x4_t,
    ti: int32x4_t,
) -> (int32x4_t, int32x4_t) {
    unsafe {
        (
            vsubq_s32(s_mul4(ar, tr), s_mul4(ai, ti)),
            vaddq_s32(s_mul4(ar, ti), s_mul4(ai, tr)),
        )
    }
}

/// Load four consecutive complexes as (r lanes, i lanes).
#[inline(always)]
unsafe fn lc4(p: *const crate::celt::fft::KissFftCpx) -> (int32x4_t, int32x4_t) {
    unsafe {
        let int32x4x2_t(r, i) = vld2q_s32(p as *const i32);
        (r, i)
    }
}

/// Store (r lanes, i lanes) back as four consecutive complexes.
#[inline(always)]
unsafe fn sc4(p: *mut crate::celt::fft::KissFftCpx, r: int32x4_t, i: int32x4_t) {
    unsafe { vst2q_s32(p as *mut i32, int32x4x2_t(r, i)) }
}

/// NEON radix-4 butterfly (twiddled path, 4-wide j-blocks). Processes the
/// `m & !3` leading j positions of each group; the caller runs the scalar
/// tail for the remaining `m % 4`. Returns `m & !3`.
pub(crate) fn kf_bfly4_neon(
    fout: &mut [crate::celt::fft::KissFftCpx],
    fstride: usize,
    twiddles: &[crate::celt::fft::KissTwiddleCpx],
    m: usize,
    n: usize,
    mm: usize,
) -> usize {
    let m2 = 2 * m;
    let m3 = 3 * m;
    let mj = m & !3;
    for i in 0..n {
        let base = i * mm;
        let mut j = 0usize;
        while j < mj {
            let f = base + j;
            unsafe {
                let (f0r, f0i) = lc4(fout.as_ptr().add(f));
                let (f1r, f1i) = lc4(fout.as_ptr().add(f + m));
                let (f2r, f2i) = lc4(fout.as_ptr().add(f + m2));
                let (f3r, f3i) = lc4(fout.as_ptr().add(f + m3));

                let (t1r, t1i) = tw4(twiddles, j * fstride, fstride);
                let (t2r, t2i) = tw4(twiddles, 2 * j * fstride, 2 * fstride);
                let (t3r, t3i) = tw4(twiddles, 3 * j * fstride, 3 * fstride);

                let (s0r, s0i) = cmul4(f1r, f1i, t1r, t1i);
                let (s1r, s1i) = cmul4(f2r, f2i, t2r, t2i);
                let (s2r, s2i) = cmul4(f3r, f3i, t3r, t3i);

                let s5r = vsubq_s32(f0r, s1r);
                let s5i = vsubq_s32(f0i, s1i);
                let a0r = vaddq_s32(f0r, s1r); // f0 += s1
                let a0i = vaddq_s32(f0i, s1i);
                let s3r = vaddq_s32(s0r, s2r);
                let s3i = vaddq_s32(s0i, s2i);
                let s4r = vsubq_s32(s0r, s2r);
                let s4i = vsubq_s32(s0i, s2i);

                sc4(
                    fout.as_mut_ptr().add(f + m2),
                    vsubq_s32(a0r, s3r),
                    vsubq_s32(a0i, s3i),
                );
                sc4(
                    fout.as_mut_ptr().add(f),
                    vaddq_s32(a0r, s3r),
                    vaddq_s32(a0i, s3i),
                );
                sc4(
                    fout.as_mut_ptr().add(f + m),
                    vaddq_s32(s5r, s4i),
                    vsubq_s32(s5i, s4r),
                );
                sc4(
                    fout.as_mut_ptr().add(f + m3),
                    vsubq_s32(s5r, s4i),
                    vaddq_s32(s5i, s4r),
                );
            }
            j += 4;
        }
    }
    mj
}

/// NEON radix-4 butterfly, degenerate `m == 1` path (no twiddles). One
/// butterfly spans 4 consecutive complexes; vectorised within the butterfly.
pub(crate) fn kf_bfly4_degenerate_neon(fout: &mut [crate::celt::fft::KissFftCpx], n: usize) {
    let s_pos_neg = unsafe { vld1q_s32([1, 1, 1, -1].as_ptr()) };
    for k in 0..n {
        let idx = k * 4;
        unsafe {
            let v0 = vld1q_s32(fout.as_ptr().add(idx) as *const i32); // a, b
            let v1 = vld1q_s32(fout.as_ptr().add(idx + 2) as *const i32); // c, d
            let sum = vaddq_s32(v0, v1); // [a+c, b+d]
            let dif = vsubq_s32(v0, v1); // [a-c, b-d]
            let a_vec = vcombine_s32(vget_low_s32(sum), vget_low_s32(dif)); // [s0,s1,d0,d1]
            let b_vec = vcombine_s32(vget_high_s32(sum), vrev64_s32(vget_high_s32(dif))); // [s2,s3,d3,d2]
            // out(a'') = [s0+s2, s1+s3]; out(b') = [d0+d3, d1-d2]
            let out01 = vmlaq_s32(a_vec, b_vec, s_pos_neg);
            // out(c') = [s0-s2, s1-s3]; out(d') = [d0-d3, d1+d2]
            let out23 = vmlsq_s32(a_vec, b_vec, s_pos_neg);
            vst1q_s32(fout.as_mut_ptr().add(idx) as *mut i32, out01);
            vst1q_s32(fout.as_mut_ptr().add(idx + 2) as *mut i32, out23);
        }
    }
}

/// NEON radix-3 butterfly (4-wide j-blocks; caller runs the scalar tail).
/// Returns `m & !3`.
pub(crate) fn kf_bfly3_neon(
    fout: &mut [crate::celt::fft::KissFftCpx],
    fstride: usize,
    twiddles: &[crate::celt::fft::KissTwiddleCpx],
    m: usize,
    n: usize,
    mm: usize,
) -> usize {
    // epi3.i = -sin(2π/3) in Q15
    let epi3_i: i32 = -crate::types::qconst32(0.86602540, 15);
    let m2 = 2 * m;
    let mj = m & !3;
    let epi = unsafe { vdupq_n_s32(epi3_i) };
    for i in 0..n {
        let base = i * mm;
        let mut j = 0usize;
        while j < mj {
            let f = base + j;
            unsafe {
                let (f0r, f0i) = lc4(fout.as_ptr().add(f));
                let (f1r, f1i) = lc4(fout.as_ptr().add(f + m));
                let (f2r, f2i) = lc4(fout.as_ptr().add(f + m2));
                let (t1r, t1i) = tw4(twiddles, j * fstride, fstride);
                let (t2r, t2i) = tw4(twiddles, 2 * j * fstride, 2 * fstride);

                let (s1r, s1i) = cmul4(f1r, f1i, t1r, t1i);
                let (s2r, s2i) = cmul4(f2r, f2i, t2r, t2i);
                let s3r = vaddq_s32(s1r, s2r);
                let s3i = vaddq_s32(s1i, s2i);
                // scratch0 = (s1 - s2) * epi3_i
                let s0r = s_mul4(vsubq_s32(s1r, s2r), epi);
                let s0i = s_mul4(vsubq_s32(s1i, s2i), epi);

                let n1r = vsubq_s32(f0r, vshrq_n_s32(s3r, 1)); // f1' = f0 - half(s3)
                let n1i = vsubq_s32(f0i, vshrq_n_s32(s3i, 1));
                let a0r = vaddq_s32(f0r, s3r); // f0 += s3
                let a0i = vaddq_s32(f0i, s3i);

                // f2' = (n1.r + s0.i, n1.i - s0.r); f1'' = (n1.r - s0.i, n1.i + s0.r)
                sc4(fout.as_mut_ptr().add(f), a0r, a0i);
                sc4(
                    fout.as_mut_ptr().add(f + m2),
                    vaddq_s32(n1r, s0i),
                    vsubq_s32(n1i, s0r),
                );
                sc4(
                    fout.as_mut_ptr().add(f + m),
                    vsubq_s32(n1r, s0i),
                    vaddq_s32(n1i, s0r),
                );
            }
            j += 4;
        }
    }
    mj
}

/// NEON radix-5 butterfly (4-wide j-blocks; caller runs the scalar tail).
/// Returns `m & !3`.
pub(crate) fn kf_bfly5_neon(
    fout: &mut [crate::celt::fft::KissFftCpx],
    fstride: usize,
    twiddles: &[crate::celt::fft::KissTwiddleCpx],
    m: usize,
    n: usize,
    mm: usize,
) -> usize {
    // ya = e^{-j2π/5}, yb = e^{-j4π/5} in Q15 (same constants as the scalar).
    let ya_r: i32 = crate::types::qconst32(0.30901699, 15);
    let ya_i: i32 = -crate::types::qconst32(0.95105652, 15);
    let yb_r: i32 = -crate::types::qconst32(0.80901699, 15);
    let yb_i: i32 = -crate::types::qconst32(0.58778525, 15);
    let ya_rv = unsafe { vdupq_n_s32(ya_r) };
    let ya_iv = unsafe { vdupq_n_s32(ya_i) };
    let yb_rv = unsafe { vdupq_n_s32(yb_r) };
    let yb_iv = unsafe { vdupq_n_s32(yb_i) };

    let mj = m & !3;
    for i in 0..n {
        let base = i * mm;
        let mut u = 0usize;
        while u < mj {
            let f0 = base + u;
            unsafe {
                let (d0r, d0i) = lc4(fout.as_ptr().add(f0));
                let (d1r, d1i) = lc4(fout.as_ptr().add(f0 + m));
                let (d2r, d2i) = lc4(fout.as_ptr().add(f0 + 2 * m));
                let (d3r, d3i) = lc4(fout.as_ptr().add(f0 + 3 * m));
                let (d4r, d4i) = lc4(fout.as_ptr().add(f0 + 4 * m));

                let (t1r, t1i) = tw4(twiddles, u * fstride, fstride);
                let (t2r, t2i) = tw4(twiddles, 2 * u * fstride, 2 * fstride);
                let (t3r, t3i) = tw4(twiddles, 3 * u * fstride, 3 * fstride);
                let (t4r, t4i) = tw4(twiddles, 4 * u * fstride, 4 * fstride);

                let (s1r, s1i) = cmul4(d1r, d1i, t1r, t1i);
                let (s2r, s2i) = cmul4(d2r, d2i, t2r, t2i);
                let (s3r, s3i) = cmul4(d3r, d3i, t3r, t3i);
                let (s4r, s4i) = cmul4(d4r, d4i, t4r, t4i);

                let s7r = vaddq_s32(s1r, s4r);
                let s7i = vaddq_s32(s1i, s4i);
                let s10r = vsubq_s32(s1r, s4r);
                let s10i = vsubq_s32(s1i, s4i);
                let s8r = vaddq_s32(s2r, s3r);
                let s8i = vaddq_s32(s2i, s3i);
                let s9r = vsubq_s32(s2r, s3r);
                let s9i = vsubq_s32(s2i, s3i);

                // f0 += s7 + s8
                sc4(
                    fout.as_mut_ptr().add(f0),
                    vaddq_s32(d0r, vaddq_s32(s7r, s8r)),
                    vaddq_s32(d0i, vaddq_s32(s7i, s8i)),
                );

                // scratch5 = f0 + s7*ya_r + s8*yb_r
                let s5r = vaddq_s32(d0r, vaddq_s32(s_mul4(s7r, ya_rv), s_mul4(s8r, yb_rv)));
                let s5i = vaddq_s32(d0i, vaddq_s32(s_mul4(s7i, ya_rv), s_mul4(s8i, yb_rv)));
                // scratch6 = (s10.i*ya_i + s9.i*yb_i, -(s10.r*ya_i + s9.r*yb_i))
                let s6r = vaddq_s32(s_mul4(s10i, ya_iv), s_mul4(s9i, yb_iv));
                let s6i = vsubq_s32(
                    vdupq_n_s32(0),
                    vaddq_s32(s_mul4(s10r, ya_iv), s_mul4(s9r, yb_iv)),
                );

                // f1 = s5 - s6; f4 = s5 + s6
                sc4(
                    fout.as_mut_ptr().add(f0 + m),
                    vsubq_s32(s5r, s6r),
                    vsubq_s32(s5i, s6i),
                );
                sc4(
                    fout.as_mut_ptr().add(f0 + 4 * m),
                    vaddq_s32(s5r, s6r),
                    vaddq_s32(s5i, s6i),
                );

                // scratch11 = f0 + s7*yb_r + s8*ya_r
                let s11r = vaddq_s32(d0r, vaddq_s32(s_mul4(s7r, yb_rv), s_mul4(s8r, ya_rv)));
                let s11i = vaddq_s32(d0i, vaddq_s32(s_mul4(s7i, yb_rv), s_mul4(s8i, ya_rv)));
                // scratch12 = (s9.i*ya_i - s10.i*yb_i, s10.r*yb_i - s9.r*ya_i)
                let s12r = vsubq_s32(s_mul4(s9i, ya_iv), s_mul4(s10i, yb_iv));
                let s12i = vsubq_s32(s_mul4(s10r, yb_iv), s_mul4(s9r, ya_iv));

                // f2 = s11 + s12; f3 = s11 - s12
                sc4(
                    fout.as_mut_ptr().add(f0 + 2 * m),
                    vaddq_s32(s11r, s12r),
                    vaddq_s32(s11i, s12i),
                );
                sc4(
                    fout.as_mut_ptr().add(f0 + 3 * m),
                    vsubq_s32(s11r, s12r),
                    vsubq_s32(s11i, s12i),
                );
            }
            u += 4;
        }
    }
    mj
}

// ===========================================================================
// CELT: MDCT post-rotation (clt_mdct_backward step 3)
// ===========================================================================

/// `s_mul(a, b) = ((b as i16) * a) >> 15` truncated to i32, on four lanes.
/// `b` must already be sign-extended from i16.
#[inline(always)]
fn s_mul4(a: int32x4_t, b: int32x4_t) -> int32x4_t {
    unsafe {
        let lo = vshrq_n_s64(vmull_s32(vget_low_s32(a), vget_low_s32(b)), 15);
        let hi = vshrq_n_s64(vmull_high_s32(a, b), 15);
        vcombine_s32(vmovn_s64(lo), vmovn_s64(hi))
    }
}

/// `pshr32_ovflw(a, shift) = (a + bias) >> shift` (wrapping add, arithmetic
/// shift) on four lanes. `neg_shift` = `vdupq_n_s32(-shift)`.
#[inline(always)]
fn pshr4(a: int32x4_t, bias: int32x4_t, neg_shift: int32x4_t) -> int32x4_t {
    unsafe { vshlq_s32(vaddq_s32(a, bias), neg_shift) }
}

/// NEON post-rotation + de-shuffle of `clt_mdct_backward` for the common
/// case where forward and backward sweep regions do not overlap within a
/// 4-iteration block; the caller keeps the scalar loop for the tail.
///
/// Returns the number of iterations completed (the caller resumes the scalar
/// loop at that `i`). Bit-exact: lane-wise products/shifts match the scalar
/// `s_mul`/`pshr32_ovflw`; adds wrap in i32.
pub(crate) fn mdct_postrotate_neon(
    output: &mut [i32],
    half_ov: usize,
    n2: usize,
    n4: usize,
    trig: &[i16],
    post_shift: i32,
) -> usize {
    let iters = (n4 + 1) >> 1;
    // Safe vector region: within a 4-iteration block the forward window
    // [yp0+2i .. yp0+2i+7] and the backward window [yp1-2i-6 .. yp1-2i+1]
    // must not overlap; the strict condition reduces to i < n4/2 - 4.
    let vec_end = iters.saturating_sub(4) & !3;
    let bias = unsafe { vdupq_n_s32(shl_bias(post_shift)) };
    let neg_shift = unsafe { vdupq_n_s32(-post_shift) };
    let mut i = 0usize;
    while i < vec_end {
        let yp0 = half_ov + 2 * i;
        let yp1 = half_ov + n2 - 2 - 2 * i;
        unsafe {
            // Forward sweep: pairs (im, re) ascending.
            let int32x4x2_t(im_v, re_v) = vld2q_s32(output.as_ptr().add(yp0));
            let t0 = vmovl_s16(vld1_s16(trig.as_ptr().add(i)));
            let t1 = vmovl_s16(vld1_s16(trig.as_ptr().add(n4 + i)));
            let yr_v = pshr4(
                vaddq_s32(s_mul4(re_v, t0), s_mul4(im_v, t1)),
                bias,
                neg_shift,
            );
            let yi_v = pshr4(
                vsubq_s32(s_mul4(re_v, t1), s_mul4(im_v, t0)),
                bias,
                neg_shift,
            );

            // Backward sweep: lane L holds iteration i+3-L (data reversed);
            // the reversed data order makes the coefficient loads contiguous
            // ascending (reversal cancels).
            let int32x4x2_t(im2_v, re2_v) = vld2q_s32(output.as_ptr().add(yp1 - 6));
            let t0b = vmovl_s16(vld1_s16(trig.as_ptr().add(n4 - 4 - i)));
            let t1b = vmovl_s16(vld1_s16(trig.as_ptr().add(n2 - 4 - i)));
            let yr2_v = pshr4(
                vaddq_s32(s_mul4(re2_v, t0b), s_mul4(im2_v, t1b)),
                bias,
                neg_shift,
            );
            let yi2_v = pshr4(
                vsubq_s32(s_mul4(re2_v, t1b), s_mul4(im2_v, t0b)),
                bias,
                neg_shift,
            );

            // Reverse the cross-stored vector so both stores use lane order.
            let rev = |v: int32x4_t| vextq_s32(vrev64q_s32(v), vrev64q_s32(v), 2);
            // output[yp0+2k] = yr_k, output[yp0+2k+1] = yi2_k (k ascending).
            let yi2_fwd = rev(yi2_v);
            vst2q_s32(output.as_mut_ptr().add(yp0), int32x4x2_t(yr_v, yi2_fwd));
            // Backward store at yp1-6: lane L writes iteration i+3-L, which
            // needs yr2 in lane order and yi reversed.
            let yi_rev = rev(yi_v);
            vst2q_s32(output.as_mut_ptr().add(yp1 - 6), int32x4x2_t(yr2_v, yi_rev));
        }
        i += 4;
    }
    i
}

/// `(1 << shift) >> 1` without overflow (shift < 31 here).
#[inline(always)]
fn shl_bias(shift: i32) -> i32 {
    ((1u32 << shift) >> 1) as i32
}

/// NEON pre-rotation of `clt_mdct_backward` for `stride == 1` with a full
/// input (`input.len() >= n2`): folds the input into N/4 rotated complex
/// pairs written in bit-reversed order. The arithmetic runs four lanes wide;
/// the bit-reversed stores stay scalar (scatter).
///
/// Returns the number of iterations completed; the caller resumes the scalar
/// loop at that `i`. Bit-exact: `shl32_ovflw` is a wrapping left shift
/// (`vshlq`), `s_mul` per lane matches the scalar 64-bit recipe, and the
/// final adds/subs wrap.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mdct_prerotate_neon(
    output: &mut [i32],
    input: &[i32],
    half_ov: usize,
    n2: usize,
    n4: usize,
    trig: &[i16],
    bitrev: &[i16],
    pre_shift: i32,
) -> usize {
    let vec_end = (n4.saturating_sub(3)) & !3;
    let shift = unsafe { vdupq_n_s32(pre_shift) };
    let mut i = 0usize;
    while i < vec_end {
        unsafe {
            // x1: forward stride-2, lanes in iteration order.
            let int32x4x2_t(x1, _) = vld2q_s32(input.as_ptr().add(2 * i));
            // x2: backward stride-2; the vld2 odd lanes come out reversed, so
            // flip them back to iteration order.
            let int32x4x2_t(_, x2r) = vld2q_s32(input.as_ptr().add(n2 - 2 * i - 8));
            let x2r = vextq_s32(vrev64q_s32(x2r), vrev64q_s32(x2r), 2);
            let x1 = vshlq_s32(x1, shift);
            let x2 = vshlq_s32(x2r, shift);

            let t0 = vmovl_s16(vld1_s16(trig.as_ptr().add(i)));
            let t1 = vmovl_s16(vld1_s16(trig.as_ptr().add(n4 + i)));
            let yr = vaddq_s32(s_mul4(x2, t0), s_mul4(x1, t1));
            let yi = vsubq_s32(s_mul4(x1, t0), s_mul4(x2, t1));

            // Bit-reversed scatter stores (scalar).
            let mut yr_a = [0i32; 4];
            let mut yi_a = [0i32; 4];
            vst1q_s32(yr_a.as_mut_ptr(), yr);
            vst1q_s32(yi_a.as_mut_ptr(), yi);
            for k in 0..4 {
                let rev = *bitrev.get_unchecked(i + k) as usize;
                *output.get_unchecked_mut(half_ov + 2 * rev) = yi_a[k];
                *output.get_unchecked_mut(half_ov + 2 * rev + 1) = yr_a[k];
            }
        }
        i += 4;
    }
    i
}


/// Whole-subframe SILK LPC synthesis with the recursive state window kept in
/// vector registers.
///
/// Per sample `i` (matching the scalar loop exactly):
///   `pred  = (order>>1) + Σ_j (state[base-1-j] * a[j]) >> 16`   (i64 sum)
///   `state[base+i] = add_sat32(res[i], lshift_sat32(pred, 4))`
///   `xq[i] = sat16(rshift_round(smulww(state[base+i], gain_q10), 8))`
///
/// Why not a per-sample NEON dot product over the memory window? A 16-byte
/// vector load partially overlapping the 4-byte store of the previous
/// sample misses store-to-load forwarding on Apple cores, making the naive
/// NEON per-sample kernel ~2x *slower* than the unrolled scalar loop
/// (measured). Keeping the 16-state window in registers removes the load
/// from the loop-carried chain entirely: the window slides with `vext` and
/// the new sample enters via one lane insert.
///
/// `a_rev16` holds the Q12 coefficients reversed and zero-padded to 16 lanes
/// (`a_rev16[k] = a[15-k]` for order 16; leading zeros then `a[9-k]` tail for
/// order 10 — see `silk::decoder::lpc_coefs_reversed16`). Bit-exact: per-lane
/// products and `>>16` shifts are identical to the scalar taps; the i64
/// accumulation tree differs from the scalar wrapping-i32 tree only in order,
/// which is irrelevant modulo 2^32.
pub(crate) fn silk_lpc_synth_subframe_neon(
    s_lpc_q14: &mut [i32],
    res_q14: &[i32],
    xq: &mut [i16],
    xq_offset: usize,
    gain_q10: i32,
    a_rev16: &[i16; 16],
    order: usize,
    subfr_length: usize,
) {
    use crate::silk::common::{silk_add_sat32, silk_lshift_sat32, silk_rshift_round, silk_smulww};
    use crate::types::sat16;
    debug_assert!(order == 10 || order == 16);
    unsafe {
        // Window registers: lane k of the concatenated [w0,w1,w2,w3] holds
        // state[base-16+k] for the current sample.
        let base_ptr = s_lpc_q14.as_mut_ptr().add(MAX_LPC_ORDER);
        let mut w0 = vld1q_s32(base_ptr.sub(16));
        let mut w1 = vld1q_s32(base_ptr.sub(12));
        let mut w2 = vld1q_s32(base_ptr.sub(8));
        let mut w3 = vld1q_s32(base_ptr.sub(4));

        let ca = vmovl_s16(vld1_s16(a_rev16.as_ptr()));
        let cb = vmovl_s16(vld1_s16(a_rev16.as_ptr().add(4)));
        let cc = vmovl_s16(vld1_s16(a_rev16.as_ptr().add(8)));
        let cd = vmovl_s16(vld1_s16(a_rev16.as_ptr().add(12)));

        let bias = (order >> 1) as i64;

        for i in 0..subfr_length {
            // 8 independent s64x2 chains (order 10 zeroes the last 4 taps via
            // the padded coefficient lanes).
            let a0 = vshrq_n_s64(vmull_s32(vget_low_s32(w0), vget_low_s32(ca)), 16);
            let a1 = vshrq_n_s64(vmull_s32(vget_high_s32(w0), vget_high_s32(ca)), 16);
            let a2 = vshrq_n_s64(vmull_s32(vget_low_s32(w1), vget_low_s32(cb)), 16);
            let a3 = vshrq_n_s64(vmull_s32(vget_high_s32(w1), vget_high_s32(cb)), 16);
            let a4 = vshrq_n_s64(vmull_s32(vget_low_s32(w2), vget_low_s32(cc)), 16);
            let a5 = vshrq_n_s64(vmull_s32(vget_high_s32(w2), vget_high_s32(cc)), 16);
            let a6 = vshrq_n_s64(vmull_s32(vget_low_s32(w3), vget_low_s32(cd)), 16);
            let a7 = vshrq_n_s64(vmull_s32(vget_high_s32(w3), vget_high_s32(cd)), 16);
            let s0 = vaddq_s64(vaddq_s64(a0, a1), vaddq_s64(a2, a3));
            let s1 = vaddq_s64(vaddq_s64(a4, a5), vaddq_s64(a6, a7));
            let s = vaddq_s64(s0, s1);
            let pred = (bias + vgetq_lane_s64(s, 0) + vgetq_lane_s64(s, 1)) as i32;

            let new = silk_add_sat32(res_q14[i], silk_lshift_sat32(pred, 4));
            s_lpc_q14[MAX_LPC_ORDER + i] = new;
            xq[xq_offset + i] = sat16(silk_rshift_round(silk_smulww(new, gain_q10), 8));

            // Slide the window left by one sample and insert `new`.
            w0 = vextq_s32(w0, w1, 1);
            w1 = vextq_s32(w1, w2, 1);
            w2 = vextq_s32(w2, w3, 1);
            w3 = vsetq_lane_s32(new, vextq_s32(w3, w3, 1), 3);
        }
    }
}


// ===========================================================================
// SILK resampler: 8-tap fractional FIR interpolation (iir_fir)
// ===========================================================================

/// Combined 8-tap interpolation table derived from `SILK_RESAMPLER_FRAC_FIR_12`.
/// Row `t` pairs with ascending `buf[buf_idx..buf_idx+8]`:
/// `[F[t][0..4], F[11-t][3], F[11-t][2], F[11-t][1], F[11-t][0]]`.
const fn build_fir12_8tap() -> [[i16; 8]; 12] {
    let f = crate::silk::tables::SILK_RESAMPLER_FRAC_FIR_12;
    let mut t8 = [[0i16; 8]; 12];
    let mut t = 0;
    while t < 12 {
        t8[t][0] = f[t][0];
        t8[t][1] = f[t][1];
        t8[t][2] = f[t][2];
        t8[t][3] = f[t][3];
        t8[t][4] = f[11 - t][3];
        t8[t][5] = f[11 - t][2];
        t8[t][6] = f[11 - t][1];
        t8[t][7] = f[11 - t][0];
        t += 1;
    }
    t8
}

/// Fractional-delay FIR interpolation loop of `silk_resampler_private_iir_fir`.
///
/// Bit-exact: i16×i16 products are exact in i32 lanes and the wrapping sum is
/// order-independent modulo 2^32; rounding/saturation stay scalar.
/// Returns the advanced `out_ptr`, matching the scalar loop's bookkeeping
/// (including its `out.len()` guard).
pub(crate) fn silk_fir12_interpolate_neon(
    buf: &[i16],
    out: &mut [i16],
    mut out_ptr: usize,
    max_index_q16: i32,
    index_increment_q16: i32,
) -> usize {
    use crate::silk::common::silk_rshift_round;
    use crate::silk::decoder::smulwb;
    use crate::types::sat16;
    const T8: [[i16; 8]; 12] = build_fir12_8tap();
    let mut index_q16 = 0i32;
    while index_q16 < max_index_q16 {
        let table_index = smulwb(index_q16 & 0xFFFF, 12) as usize;
        let buf_idx = (index_q16 >> 16) as usize;
        debug_assert!(table_index < 12);
        debug_assert!(buf_idx + 8 <= buf.len());
        let res_q15 = unsafe {
            let b = vld1q_s16(buf.as_ptr().add(buf_idx));
            let c = vld1q_s16(T8[table_index].as_ptr());
            let sum = vaddq_s32(vmull_s16(vget_low_s16(b), vget_low_s16(c)), vmull_high_s16(b, c));
            vaddvq_s32(sum)
        };
        if out_ptr < out.len() {
            out[out_ptr] = sat16(silk_rshift_round(res_q15, 15));
        }
        out_ptr += 1;
        index_q16 += index_increment_q16;
    }
    out_ptr
}

// ===========================================================================
// SILK resampler: 2x high-quality allpass upsampler (up2_hq)
// ===========================================================================

/// NEON version of `silk_resampler_private_up2_hq`. The even and odd output
/// paths are two independent 3-stage allpass chains fed by the same input
/// sample, so they run in the two lanes of an s32x2 (i64x2 for the widening
/// multiplies). Bit-exact: `smulwb` per stage is a 64-bit product truncated
/// after `>>16` (`vmovn` reproduces `as i32`), adds wrap in i32, and the
/// final `sat16` is a saturating narrow (`vqmovn`).
pub(crate) fn silk_up2_hq_neon(s: &mut [i32; 6], out: &mut [i16], input: &[i16], len: usize) {
    // Allpass coefficients (from resampler_rom.h), interleaved per stage.
    const UP2_HQ_0: [i32; 3] = [1746, 14986, -26453]; // Even path
    const UP2_HQ_1: [i32; 3] = [6854, 25769, -9994]; // Odd path

    // Lane 0 = even path, lane 1 = odd path.
    unsafe {
    let coef0 = vld1_s32([UP2_HQ_0[0], UP2_HQ_1[0]].as_ptr());
    let coef1 = vld1_s32([UP2_HQ_0[1], UP2_HQ_1[1]].as_ptr());
    let coef2 = vld1_s32([UP2_HQ_0[2], UP2_HQ_1[2]].as_ptr());
    let mut st0 = vld1_s32([s[0], s[3]].as_ptr());
    let mut st1 = vld1_s32([s[1], s[4]].as_ptr());
    let mut st2 = vld1_s32([s[2], s[5]].as_ptr());
    let one = vdup_n_s32(1);

    /// `(y * c) >> 16` truncated to i32, per lane.
    #[inline(always)]
    fn smulwb_v(y: int32x2_t, c: int32x2_t) -> int32x2_t {
        unsafe { vmovn_s64(vshrq_n_s64(vmull_s32(y, c), 16)) }
    }

    for k in 0..len {
        let in32 = vdup_n_s32(i32::from(input[k]) << 10);

        let y = vsub_s32(in32, st0);
        let x = smulwb_v(y, coef0);
        let out_a = vadd_s32(st0, x);
        st0 = vadd_s32(in32, x);

        let y = vsub_s32(out_a, st1);
        let x = smulwb_v(y, coef1);
        let out_b = vadd_s32(st1, x);
        st1 = vadd_s32(out_a, x);

        // Third stage folds `y +` into x (matches the scalar path).
        let y = vsub_s32(out_b, st2);
        let x = vadd_s32(y, smulwb_v(y, coef2));
        let out_c = vadd_s32(st2, x);
        st2 = vadd_s32(out_b, x);

        // sat16(rshift_round(v, 10)) per lane: ((v>>9)+1)>>1, then i16 clamp.
        let r = vshr_n_s32(vadd_s32(vshr_n_s32(out_c, 9), one), 1);
        let r16 = vqmovn_s32(vcombine_s32(r, vdup_n_s32(0)));
        out[2 * k] = vget_lane_s16(r16, 0);
        out[2 * k + 1] = vget_lane_s16(r16, 1);
    }

    s[0] = vget_lane_s32(st0, 0);
    s[3] = vget_lane_s32(st0, 1);
    s[1] = vget_lane_s32(st1, 0);
    s[4] = vget_lane_s32(st1, 1);
    s[2] = vget_lane_s32(st2, 0);
    s[5] = vget_lane_s32(st2, 1);
    }
}

// ===========================================================================
// CELT: MDCT dynamic-range scan (maxval / sumval)
// ===========================================================================

/// NEON version of the `clt_mdct_backward` dynamic-range scan for `stride==1`:
/// `maxval = max(maxval, abs32(x))`, `sumval += abs32(x >> 11)` (wrapping).
///
/// `abs32` matches `vabsq_s32` even for `i32::MIN` (both yield `i32::MIN`).
/// Zero-valued samples beyond `input_len` contribute nothing, so scanning
/// `min(n2, input_len)` is exact. Returns `(maxval, sum_delta)`; the caller
/// wrapping-adds `sum_delta` to its `sumval` seed (order-independent).
pub(crate) fn mdct_norm_scan_neon(input: &[i32], n2: usize, input_len: usize) -> (i32, i32) {
    let n_eff = n2.min(input_len);
    let chunks = n_eff / 4;
    unsafe {
    let mut maxv = vdupq_n_s32(0);
    let mut sumv = vdupq_n_s32(0);
    for c in 0..chunks {
        let v = vld1q_s32(input.as_ptr().add(c * 4));
        maxv = vmaxq_s32(maxv, vabsq_s32(v));
        sumv = vaddq_s32(sumv, vabsq_s32(vshrq_n_s32(v, 11)));
    }
    let mut maxval = vmaxvq_s32(maxv);
    let mut sum_delta: i32 = vaddvq_s32(sumv);
    for i in chunks * 4..n_eff {
        let sample = input[i];
        let a = sample.wrapping_neg().max(sample); // abs32, MIN-safe
        if a > maxval {
            maxval = a;
        }
        let d = sample >> 11;
        let ad = d.wrapping_neg().max(d);
        sum_delta = sum_delta.wrapping_add(ad);
    }
    (maxval, sum_delta)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> i32 {
        *seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        *seed as i32
    }

    // ---- silk_up2_hq_neon ----

    #[test]
    fn up2_hq_neon_matches_scalar() {
        let mut rng: u32 = 0x1234_5678;
        for len in [1usize, 2, 3, 7, 40, 160, 320] {
            let input: Vec<i16> = (0..len).map(|_| lcg(&mut rng) as i16).collect();
            let init: [i32; 6] = [
                lcg(&mut rng) % 1000,
                lcg(&mut rng) % 1000,
                lcg(&mut rng) % 1000,
                lcg(&mut rng) % 1000,
                lcg(&mut rng) % 1000,
                lcg(&mut rng) % 1000,
            ];
            let mut s_scalar = init;
            let mut s_neon = init;
            let mut out_scalar = vec![0i16; 2 * len];
            let mut out_neon = vec![0i16; 2 * len];
            crate::silk::decoder::silk_resampler_private_up2_hq(
                &mut s_scalar,
                &mut out_scalar,
                &input,
                len,
            );
            silk_up2_hq_neon(&mut s_neon, &mut out_neon, &input, len);
            assert_eq!(out_scalar, out_neon, "output mismatch at len={len}");
            assert_eq!(s_scalar, s_neon, "state mismatch at len={len}");
        }
    }

    // ---- silk_fir12_interpolate_neon ----

    #[test]
    fn fir12_interpolate_neon_matches_scalar() {
        let mut rng: u32 = 0xCAFE_0001;
        for (max_index, step) in [
            (640 << 16, 32768i32),
            (960 << 16, 21845),
            (480 << 16, 43691),
            (320 << 16, 65536),
        ] {
            let buf: Vec<i16> = (0..1100).map(|_| lcg(&mut rng) as i16).collect();
            let mut out_scalar = vec![0i16; 1400];
            let mut out_neon = vec![0i16; 1400];
            let n_scalar = crate::silk::decoder::fir12_interpolate_scalar(
                &buf,
                &mut out_scalar,
                0,
                max_index,
                step,
            );
            let n_neon = silk_fir12_interpolate_neon(&buf, &mut out_neon, 0, max_index, step);
            assert_eq!(n_scalar, n_neon);
            assert_eq!(out_scalar, out_neon, "mismatch step={step}");
        }
    }

    // ---- mdct_norm_scan_neon ----

    #[test]
    fn mdct_norm_scan_neon_matches_scalar() {
        let mut rng: u32 = 0xABCD_1234;
        for n2 in [0usize, 1, 2, 3, 4, 5, 60, 120, 240, 480, 960] {
            for input_len in [n2, n2 / 2, 0] {
                let input: Vec<i32> = (0..input_len).map(|_| lcg(&mut rng)).collect();
                let (maxv_s, sumd_s) = crate::celt::mdct::norm_scan_scalar(&input, n2, 1);
                let (maxv_n, sumd_n) = mdct_norm_scan_neon(&input, n2, input_len);
                assert_eq!(
                    (maxv_s, sumd_s),
                    (maxv_n, sumd_n),
                    "mismatch at n2={n2} input_len={input_len}"
                );
            }
        }
        // i32::MIN edge: abs32(i32::MIN) wraps to i32::MIN in both paths.
        let input = [i32::MIN, 0, i32::MAX, -1, 1, i32::MIN + 1];
        let a = crate::celt::mdct::norm_scan_scalar(&input, 6, 1);
        let b = mdct_norm_scan_neon(&input, 6, 6);
        assert_eq!(a, b);
    }

    // ---- comb_filter_const_neon / stereo_merge_neon / inner_prod ----

    #[test]
    fn comb_filter_const_neon_matches_scalar() {
        use crate::types::{mult16_32_q15, SIG_SAT};
        let mut rng: u32 = 0x4242_8888;
        for &(t1, count) in &[(15usize, 960usize), (20, 300), (17, 7), (15, 3)] {
            let off = t1 + 8;
            let mut buf0: Vec<i32> = (0..off + count + 8).map(|_| lcg(&mut rng) >> 6).collect();
            let expect = {
                let mut b = buf0.clone();
                for i in 0..count {
                    let idx = off + i;
                    let val = b[idx]
                        .wrapping_add(mult16_32_q15(1000, b[idx - t1]))
                        .wrapping_add(mult16_32_q15(
                            2000,
                            b[idx + 1 - t1].wrapping_add(b[idx - 1 - t1]),
                        ))
                        .wrapping_add(mult16_32_q15(
                            500,
                            b[idx + 2 - t1].wrapping_add(b[idx - 2 - t1]),
                        ));
                    b[idx] = crate::types::saturate(val.wrapping_sub(1), SIG_SAT);
                }
                b
            };
            let done = comb_filter_const_neon(&mut buf0, off, count, t1, 1000, 2000, 500);
            for i in done..count {
                let idx = off + i;
                let val = buf0[idx]
                    .wrapping_add(mult16_32_q15(1000, buf0[idx - t1]))
                    .wrapping_add(mult16_32_q15(
                        2000,
                        buf0[idx + 1 - t1].wrapping_add(buf0[idx - 1 - t1]),
                    ))
                    .wrapping_add(mult16_32_q15(
                        500,
                        buf0[idx + 2 - t1].wrapping_add(buf0[idx - 2 - t1]),
                    ));
                buf0[idx] = crate::types::saturate(val.wrapping_sub(1), SIG_SAT);
            }
            assert_eq!(expect, buf0, "mismatch t1={t1} count={count}");
        }
    }

    #[test]
    fn stereo_merge_neon_matches_scalar() {
        use crate::types::{mult32_32_q31, vshr32};
        let mut rng: u32 = 0x7777_3333;
        for n in [1usize, 3, 4, 5, 21, 96] {
            let x0: Vec<i32> = (0..n).map(|_| lcg(&mut rng) >> 3).collect();
            let y0: Vec<i32> = (0..n).map(|_| lcg(&mut rng) >> 3).collect();
            let (mid, lgain, rgain, kl, kr) = (100000, 26000, 27000, 8, 9);
            let mut xs = x0.clone();
            let mut ys = y0.clone();
            for j in 0..n {
                let l = mult32_32_q31(mid, xs[j]);
                let r = ys[j];
                xs[j] = vshr32(mult32_32_q31(lgain, l.wrapping_sub(r)), kl - 15);
                ys[j] = vshr32(mult32_32_q31(rgain, l.wrapping_add(r)), kr - 15);
            }
            let mut xn = x0.clone();
            let mut yn = y0.clone();
            stereo_merge_neon(&mut xn, &mut yn, mid, lgain, rgain, kl, kr, n);
            assert_eq!(xs, xn, "x mismatch n={n}");
            assert_eq!(ys, yn, "y mismatch n={n}");
        }
    }

    #[test]
    fn inner_prod_norm_shift_neon_matches_scalar() {
        let mut rng: u32 = 0x9999_5551;
        for n in [0usize, 1, 3, 4, 5, 21, 96, 960] {
            let x: Vec<i32> = (0..n).map(|_| lcg(&mut rng)).collect();
            let y: Vec<i32> = (0..n).map(|_| lcg(&mut rng)).collect();
            let mut sum: i64 = 0;
            for i in 0..n {
                sum = sum.wrapping_add(x[i] as i64 * y[i] as i64);
            }
            let want = (sum >> (2 * (crate::types::NORM_SHIFT - 14))) as i32;
            let got = celt_inner_prod_norm_shift_neon(&x, &y, n);
            assert_eq!(got, want, "mismatch n={n}");
        }
    }

    // ---- mdct_prerotate_neon ----

    #[test]
    fn mdct_prerotate_neon_matches_scalar() {
        use crate::types::{mult16_32_q15, add32_ovflw, sub32_ovflw, shl32};
        let mut rng: u32 = 0x8642_1357;
        // Deterministic bit-reversal-style permutation for n4.
        let mk_bitrev = |n4: usize| -> Vec<i16> {
            let mut v: Vec<i16> = (0..n4 as i16).collect();
            // Simple deterministic shuffle (bit-reversal properties don't
            // matter for correctness of the kernel; any permutation does).
            let mut s: u32 = 0x1111_2222;
            for i in (1..n4).rev() {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                let j = (s as usize) % (i + 1);
                v.swap(i, j);
            }
            v
        };
        for &(n4, pre_shift) in &[(480usize, 3i32), (240, 0), (120, 5), (60, 1)] {
            let n2 = 2 * n4;
            let half_ov = 60usize;
            let out_len = half_ov + n2 + 8;
            let trig: Vec<i16> = (0..n2).map(|_| (lcg(&mut rng) >> 16) as i16).collect();
            let input: Vec<i32> = (0..n2).map(|_| lcg(&mut rng) >> 5).collect();
            let bitrev = mk_bitrev(n4);
            let init: Vec<i32> = (0..out_len).map(|_| lcg(&mut rng)).collect();

            let s_mul = |a: i32, b: i32| mult16_32_q15(b, a);
            // Scalar reference over the whole range.
            let mut expect = init.clone();
            for i in 0..n4 {
                let rev = bitrev[i] as usize;
                let x1 = shl32(input[2 * i], pre_shift);
                let x2 = shl32(input[n2 - 1 - 2 * i], pre_shift);
                let yr = add32_ovflw(s_mul(x2, trig[i] as i32), s_mul(x1, trig[n4 + i] as i32));
                let yi = sub32_ovflw(s_mul(x1, trig[i] as i32), s_mul(x2, trig[n4 + i] as i32));
                expect[half_ov + 2 * rev + 1] = yr;
                expect[half_ov + 2 * rev] = yi;
            }

            let mut actual = init.clone();
            let done = mdct_prerotate_neon(
                &mut actual, &input, half_ov, n2, n4, &trig, &bitrev, pre_shift,
            );
            for i in done..n4 {
                let rev = bitrev[i] as usize;
                let x1 = shl32(input[2 * i], pre_shift);
                let x2 = shl32(input[n2 - 1 - 2 * i], pre_shift);
                let yr = add32_ovflw(s_mul(x2, trig[i] as i32), s_mul(x1, trig[n4 + i] as i32));
                let yi = sub32_ovflw(s_mul(x1, trig[i] as i32), s_mul(x2, trig[n4 + i] as i32));
                actual[half_ov + 2 * rev + 1] = yr;
                actual[half_ov + 2 * rev] = yi;
            }
            assert_eq!(expect, actual, "mismatch at n4={n4} pre_shift={pre_shift}");
        }
    }

    // ---- mdct_postrotate_neon ----

    #[test]
    fn mdct_postrotate_neon_matches_scalar() {
        use crate::types::{mult16_32_q15, pshr32_ovflw, add32_ovflw, sub32_ovflw};
        let mut rng: u32 = 0x1357_2468;
        // (half_ov, n2) pairs covering the four shift levels.
        for &(n4, post_shift) in &[(480usize, 3i32), (240, 2), (120, 1), (60, 0), (480, 0)] {
            let n2 = 2 * n4;
            let half_ov = 60usize;
            let out_len = half_ov + n2;
            let trig: Vec<i16> = (0..n2).map(|_| (lcg(&mut rng) >> 16) as i16).collect();
            let data: Vec<i32> = (0..out_len).map(|_| lcg(&mut rng) >> 3).collect();

            // Scalar reference: full loop.
            let mut expect = data.clone();
            {
                let iters = (n4 + 1) >> 1;
                let mut yp0 = half_ov;
                let mut yp1 = half_ov + n2 - 2;
                for i in 0..iters {
                    let s_mul = |a: i32, b: i32| mult16_32_q15(b, a);
                    let re = expect[yp0 + 1];
                    let im = expect[yp0];
                    let t0 = trig[i] as i32;
                    let t1 = trig[n4 + i] as i32;
                    let yr = pshr32_ovflw(add32_ovflw(s_mul(re, t0), s_mul(im, t1)), post_shift);
                    let yi = pshr32_ovflw(sub32_ovflw(s_mul(re, t1), s_mul(im, t0)), post_shift);
                    let re2 = expect[yp1 + 1];
                    let im2 = expect[yp1];
                    expect[yp0] = yr;
                    expect[yp1 + 1] = yi;
                    let t0b = trig[n4 - i - 1] as i32;
                    let t1b = trig[n2 - i - 1] as i32;
                    let yr2 =
                        pshr32_ovflw(add32_ovflw(s_mul(re2, t0b), s_mul(im2, t1b)), post_shift);
                    let yi2 =
                        pshr32_ovflw(sub32_ovflw(s_mul(re2, t1b), s_mul(im2, t0b)), post_shift);
                    expect[yp1] = yr2;
                    expect[yp0 + 1] = yi2;
                    yp0 += 2;
                    yp1 -= 2;
                }
            }

            // NEON up to vec_end, then scalar tail (mirrors the dispatch).
            let mut actual = data.clone();
            let done = mdct_postrotate_neon(&mut actual, half_ov, n2, n4, &trig, post_shift);
            {
                let iters = (n4 + 1) >> 1;
                let mut yp0 = half_ov + 2 * done;
                let mut yp1 = half_ov + n2 - 2 - 2 * done;
                for i in done..iters {
                    let s_mul = |a: i32, b: i32| mult16_32_q15(b, a);
                    let re = actual[yp0 + 1];
                    let im = actual[yp0];
                    let t0 = trig[i] as i32;
                    let t1 = trig[n4 + i] as i32;
                    let yr = pshr32_ovflw(add32_ovflw(s_mul(re, t0), s_mul(im, t1)), post_shift);
                    let yi = pshr32_ovflw(sub32_ovflw(s_mul(re, t1), s_mul(im, t0)), post_shift);
                    let re2 = actual[yp1 + 1];
                    let im2 = actual[yp1];
                    actual[yp0] = yr;
                    actual[yp1 + 1] = yi;
                    let t0b = trig[n4 - i - 1] as i32;
                    let t1b = trig[n2 - i - 1] as i32;
                    let yr2 =
                        pshr32_ovflw(add32_ovflw(s_mul(re2, t0b), s_mul(im2, t1b)), post_shift);
                    let yi2 =
                        pshr32_ovflw(sub32_ovflw(s_mul(re2, t1b), s_mul(im2, t0b)), post_shift);
                    actual[yp1] = yr2;
                    actual[yp0 + 1] = yi2;
                    yp0 += 2;
                    yp1 -= 2;
                }
            }
            assert_eq!(expect, actual, "mismatch at n4={n4} post_shift={post_shift}");
        }
    }

    // ---- LPC / LTP kernels ----

    #[test]
    fn lpc_synth_subframe_neon_matches_scalar() {
        let mut rng: u32 = 0x2468_BD02;
        for order in [10usize, 16] {
            for subfr in [20usize, 40, 60] {
                for gain in [0i32, 1, 1000, 32768, 65535] {
                    let mut state0 = vec![0i32; MAX_LPC_ORDER + subfr];
                    for v in state0.iter_mut() {
                        *v = lcg(&mut rng) >> 4;
                    }
                    let res: Vec<i32> = (0..subfr).map(|_| lcg(&mut rng) >> 6).collect();
                    let a: Vec<i16> = (0..order).map(|_| (lcg(&mut rng) >> 17) as i16).collect();
                    let mut a_rev16 = [0i16; 16];
                    for j in 0..order {
                        a_rev16[15 - j] = a[j];
                    }
                    let mut st_s = state0.clone();
                    let mut st_n = state0.clone();
                    let mut xq_s = vec![0i16; subfr];
                    let mut xq_n = vec![0i16; subfr];
                    crate::silk::decoder::silk_lpc_synth_subframe_scalar(
                        &mut st_s, &res, &mut xq_s, 0, gain, &a, order, subfr,
                    );
                    silk_lpc_synth_subframe_neon(
                        &mut st_n, &res, &mut xq_n, 0, gain, &a_rev16, order, subfr,
                    );
                    assert_eq!(st_s, st_n, "state mismatch order={order} subfr={subfr} gain={gain}");
                    assert_eq!(xq_s, xq_n, "xq mismatch order={order} subfr={subfr} gain={gain}");
                }
            }
        }
    }

    #[test]
    fn ltp_pred_neon_matches_scalar() {
        let mut rng: u32 = 0x9999_3333;
        for _trial in 0..200 {
            let len = 64usize;
            let mut state = vec![0i32; len];
            for v in state.iter_mut() {
                *v = lcg(&mut rng) >> 4;
            }
            let b: Vec<i16> = (0..5).map(|_| (lcg(&mut rng) >> 16) as i16).collect();
            let mut b_rev = [0i16; 8];
            for k in 0..5 {
                b_rev[k] = b[4 - k];
            }
            let p = 5 + (lcg(&mut rng) as usize) % (len - 6);
            let got = silk_ltp_pred_neon(&state, &b_rev, p as i64);
            let mut want: i64 = 0;
            for j in 0..5 {
                want += (state[p - j] as i64 * b[j] as i64) >> 16;
            }
            assert_eq!(got, want as i32, "p={p}");
        }
    }
}

// ===========================================================================
// Kernel microbenchmarks (run manually):
//   cargo test -p opus-core --release --features neon2 neon2::bench -- --ignored --nocapture
// ===========================================================================

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    fn lcg(seed: &mut u32) -> i32 {
        *seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        *seed as i32
    }

    fn timeit<F: FnMut()>(label: &str, mut f: F, iters: u32) {
        // warmup
        for _ in 0..iters / 10 + 1 {
            f();
        }
        let t0 = Instant::now();
        for _ in 0..iters {
            f();
        }
        let dt = t0.elapsed().as_nanos() as f64 / iters as f64;
        println!("{label:44} {dt:10.1} ns/iter");
    }

    #[test]
    #[ignore]
    fn bench_kernels() {
        let mut rng: u32 = 0x1357_9BDF;

        // ---- SILK LPC synthesis: one subframe (40 samples), order 16 ----
        let subfr = 40usize;
        let state0: Vec<i32> = (0..MAX_LPC_ORDER + subfr).map(|_| lcg(&mut rng) >> 4).collect();
        let res: Vec<i32> = (0..subfr).map(|_| lcg(&mut rng) >> 6).collect();
        let mut xq = vec![0i16; subfr];
        let a: Vec<i16> = (0..16).map(|_| (lcg(&mut rng) >> 17) as i16).collect();
        let mut a_rev16 = [0i16; 16];
        for k in 0..16 {
            a_rev16[15 - k] = a[k];
        }
        timeit("lpc_synth_subframe scalar (40,ord16)", || {
            let mut st = std::hint::black_box(state0.clone());
            crate::silk::decoder::silk_lpc_synth_subframe_scalar(
                std::hint::black_box(&mut st),
                std::hint::black_box(&res),
                std::hint::black_box(&mut xq),
                0,
                1000,
                &a,
                16,
                subfr,
            );
            std::hint::black_box(st[MAX_LPC_ORDER]);
        }, 50_000);
        timeit("lpc_synth_subframe neon2 (40,ord16)", || {
            let mut st = std::hint::black_box(state0.clone());
            silk_lpc_synth_subframe_neon(
                std::hint::black_box(&mut st),
                std::hint::black_box(&res),
                std::hint::black_box(&mut xq),
                0,
                1000,
                std::hint::black_box(&a_rev16),
                16,
                subfr,
            );
            std::hint::black_box(st[MAX_LPC_ORDER]);
        }, 50_000);

        // ---- FIR12 interpolation: 640 outputs (16k->48k 20ms batch) ----
        let buf: Vec<i16> = (0..1100).map(|_| lcg(&mut rng) as i16).collect();
        let mut out = vec![0i16; 1400];
        timeit("fir12_interp scalar (640 out)", || {
            let o = std::hint::black_box(&mut out);
            std::hint::black_box(crate::silk::decoder::fir12_interpolate_scalar(
                std::hint::black_box(&buf),
                o,
                0,
                640 << 16,
                32768,
            ));
        }, 50_000);
        timeit("fir12_interp neon2 (640 out)", || {
            let o = std::hint::black_box(&mut out);
            std::hint::black_box(silk_fir12_interpolate_neon(
                std::hint::black_box(&buf),
                o,
                0,
                640 << 16,
                32768,
            ));
        }, 50_000);

        // ---- up2_hq: 320 input samples (16k 20ms) ----
        let input: Vec<i16> = (0..320).map(|_| lcg(&mut rng) as i16).collect();
        let mut s_scalar = [0i32; 6];
        let mut s_neon = [0i32; 6];
        let mut out2 = vec![0i16; 640];
        timeit("up2_hq scalar (320 in)", || {
            crate::silk::decoder::silk_resampler_private_up2_hq(
                std::hint::black_box(&mut s_scalar),
                std::hint::black_box(&mut out2),
                std::hint::black_box(&input),
                320,
            );
        }, 20_000);
        timeit("up2_hq neon2 (320 in)", || {
            silk_up2_hq_neon(
                std::hint::black_box(&mut s_neon),
                std::hint::black_box(&mut out2),
                std::hint::black_box(&input),
                320,
            );
        }, 20_000);

        // ---- opus_fft_impl: 480-point, downshift=0 ----
        let st = &crate::celt::fft::FFT_STATE_48000_960_0;
        let f0: Vec<crate::celt::fft::KissFftCpx> = (0..480)
            .map(|_| crate::celt::fft::KissFftCpx {
                r: lcg(&mut rng) >> 12,
                i: lcg(&mut rng) >> 12,
            })
            .collect();
        let mut fbuf = f0.clone();
        timeit("opus_fft_impl 480 (current build)", || {
            let f = std::hint::black_box(&mut fbuf);
            f.copy_from_slice(std::hint::black_box(&f0));
            crate::celt::fft::opus_fft_impl(st, f, 0);
            std::hint::black_box(f[0]);
        }, 20_000);

        // ---- mdct norm scan: n2=960 ----
        let inp: Vec<i32> = (0..960).map(|_| lcg(&mut rng)).collect();
        timeit("mdct_norm_scan scalar (960)", || {
            std::hint::black_box(crate::celt::mdct::norm_scan_scalar(
                std::hint::black_box(&inp),
                960,
                1,
            ));
        }, 100_000);
        timeit("mdct_norm_scan neon2 (960)", || {
            std::hint::black_box(mdct_norm_scan_neon(std::hint::black_box(&inp), 960, 960));
        }, 100_000);
    }
}
