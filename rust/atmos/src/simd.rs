//! Packed transfer arithmetic; mixtures retain their existing 20-element/lazy storage.
use crate::catalog::GAS_COUNT;

pub(crate) struct Transfers {
    pub deltas: [f32; GAS_COUNT],
    // Test quantized values before coefficient multiplication, as DM does (including zero coefficients).
    pub active: u32,
}

pub(crate) fn calculate(
    own: &[f32; GAS_COUNT],
    other: &[f32; GAS_COUNT],
    ca: f32,
    cb: f32,
) -> Option<Transfers> {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if std::is_x86_feature_detected!("avx2") {
        // Array lengths prove every load/store is in bounds. The function accepts unaligned arrays;
        // 16 elements use AVX and the final four use SSE, without reading padding or allocating gas.
        return Some(unsafe { avx2(own, other, ca, cb) });
    }
    let _ = (own, other, ca, cb);
    None
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn avx2(own: &[f32; GAS_COUNT], other: &[f32; GAS_COUNT], ca: f32, cb: f32) -> Transfers {
    use crate::catalog::MOLAR_ACCURACY;
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    // A catalog change needs a reviewed tail, rather than an out-of-bounds SIMD load.
    const { assert!(GAS_COUNT == 20) };
    let mut output = Transfers {
        deltas: [0.0; GAS_COUNT],
        active: 0,
    };
    let zero = _mm256_setzero_ps();
    let accuracy = _mm256_set1_ps(MOLAR_ACCURACY);
    for offset in [0, 8] {
        let difference = _mm256_sub_ps(
            _mm256_loadu_ps(own.as_ptr().add(offset)),
            _mm256_loadu_ps(other.as_ptr().add(offset)),
        );
        // Preserve scalar operation boundaries; do not replace division or introduce a fused multiply-add.
        let quantized = _mm256_mul_ps(
            _mm256_floor_ps(_mm256_add_ps(
                _mm256_div_ps(difference, accuracy),
                _mm256_set1_ps(0.5),
            )),
            accuracy,
        );
        let coefficient = _mm256_blendv_ps(
            _mm256_set1_ps(cb),
            _mm256_set1_ps(ca),
            _mm256_cmp_ps::<_CMP_GT_OQ>(quantized, zero),
        );
        _mm256_storeu_ps(
            output.deltas.as_mut_ptr().add(offset),
            _mm256_mul_ps(quantized, coefficient),
        );
        output.active |=
            (_mm256_movemask_ps(_mm256_cmp_ps::<_CMP_NEQ_UQ>(quantized, zero)) as u32) << offset;
    }
    let difference = _mm_sub_ps(
        _mm_loadu_ps(own.as_ptr().add(16)),
        _mm_loadu_ps(other.as_ptr().add(16)),
    );
    let accuracy = _mm_set1_ps(MOLAR_ACCURACY);
    let quantized = _mm_mul_ps(
        _mm_floor_ps(_mm_add_ps(
            _mm_div_ps(difference, accuracy),
            _mm_set1_ps(0.5),
        )),
        accuracy,
    );
    let coefficient = _mm_blendv_ps(
        _mm_set1_ps(cb),
        _mm_set1_ps(ca),
        _mm_cmpgt_ps(quantized, _mm_setzero_ps()),
    );
    _mm_storeu_ps(
        output.deltas.as_mut_ptr().add(16),
        _mm_mul_ps(quantized, coefficient),
    );
    output.active |= (_mm_movemask_ps(_mm_cmpneq_ps(quantized, _mm_setzero_ps())) as u32) << 16;
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog::MOLAR_ACCURACY, quantize};

    #[test]
    fn packed_transfer_matches_scalar_bits_including_tail_rounding_and_zero_coefficients() {
        let mut seed = 220_u32;
        for index in 0..2048 {
            let mut own = [0.0; GAS_COUNT];
            let mut other = own;
            for gas in 0..GAS_COUNT {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                own[gas] = (seed % 1_000_000) as f32 * MOLAR_ACCURACY;
                other[gas] = own[gas] + (gas as f32 - 10.0) * MOLAR_ACCURACY / 2.0;
            }
            own[index % GAS_COUNT] = -0.0;
            let ca = [0.0, 0.2, 1.0 / 7.0, 1.0][index % 4];
            let cb = [1.0, 0.0, 0.5, 1.0 / 6.0][index % 4];
            let Some(result) = calculate(&own, &other, ca, cb) else {
                return;
            };
            for gas in 0..GAS_COUNT {
                let delta = quantize(own[gas] - other[gas]);
                assert_eq!(result.active & (1 << gas) != 0, delta != 0.0);
                if delta != 0.0 {
                    let expected = delta * if delta > 0.0 { ca } else { cb };
                    assert_eq!(
                        result.deltas[gas].to_bits(),
                        expected.to_bits(),
                        "iteration {index}, gas {gas}"
                    );
                }
            }
        }
    }
}
