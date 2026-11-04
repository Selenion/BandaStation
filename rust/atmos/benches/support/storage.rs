use bandastation_atmos::catalog::{GAS_COUNT, MOLAR_ACCURACY};

#[derive(Clone)]
pub struct TileAos {
    pub moles: [f32; GAS_COUNT],
    pub temperature: f32,
}

#[repr(C, align(32))]
#[derive(Clone)]
pub struct TileSimd {
    pub moles: [f32; 24],
    pub temperature: f32,
}

pub trait Storage: Clone {
    fn new(count: usize) -> Self;
    fn get(&self, id: usize, gas: usize) -> f32;
    fn set(&mut self, id: usize, gas: usize, value: f32);
    fn temperature(&self, id: usize) -> f32;
    fn set_temperature(&mut self, id: usize, temperature: f32);
    fn bytes(&self) -> usize;

    fn transfer(
        &mut self,
        archived: &Self,
        a: usize,
        b: usize,
        keys: &[u8],
        ca: f32,
        cb: f32,
    ) -> [f32; GAS_COUNT] {
        let mut deltas = [0.0; GAS_COUNT];
        for &gas in keys {
            let gas = gas as usize;
            let delta = archived.get(a, gas) - archived.get(b, gas);
            let mut delta = (delta / MOLAR_ACCURACY + 0.5).floor() * MOLAR_ACCURACY;
            if delta == 0.0 {
                continue;
            }
            delta *= if delta > 0.0 { ca } else { cb };
            self.set(a, gas, self.get(a, gas) - delta);
            self.set(b, gas, self.get(b, gas) + delta);
            deltas[gas] = delta;
        }
        deltas
    }
}

#[derive(Clone)]
pub struct Aos(pub Vec<TileAos>);

impl Storage for Aos {
    fn new(count: usize) -> Self {
        Self(vec![
            TileAos {
                moles: [0.0; GAS_COUNT],
                temperature: 0.0
            };
            count
        ])
    }
    #[inline(always)]
    fn get(&self, id: usize, gas: usize) -> f32 {
        self.0[id].moles[gas]
    }
    #[inline(always)]
    fn set(&mut self, id: usize, gas: usize, value: f32) {
        self.0[id].moles[gas] = value;
    }
    #[inline(always)]
    fn temperature(&self, id: usize) -> f32 {
        self.0[id].temperature
    }
    #[inline(always)]
    fn set_temperature(&mut self, id: usize, value: f32) {
        self.0[id].temperature = value;
    }
    fn bytes(&self) -> usize {
        self.0.capacity() * std::mem::size_of::<TileAos>()
    }
}

#[derive(Clone)]
pub struct Simd(pub Vec<TileSimd>);

impl Storage for Simd {
    fn new(count: usize) -> Self {
        assert!(
            avx2_available(),
            "AVX2 is unavailable; SIMD timings must not use a scalar fallback"
        );
        Self(vec![
            TileSimd {
                moles: [0.0; 24],
                temperature: 0.0
            };
            count
        ])
    }
    #[inline(always)]
    fn get(&self, id: usize, gas: usize) -> f32 {
        self.0[id].moles[gas]
    }
    #[inline(always)]
    fn set(&mut self, id: usize, gas: usize, value: f32) {
        self.0[id].moles[gas] = value;
    }
    #[inline(always)]
    fn temperature(&self, id: usize) -> f32 {
        self.0[id].temperature
    }
    #[inline(always)]
    fn set_temperature(&mut self, id: usize, value: f32) {
        self.0[id].temperature = value;
    }
    fn bytes(&self) -> usize {
        self.0.capacity() * std::mem::size_of::<TileSimd>()
    }
    fn transfer(
        &mut self,
        archived: &Self,
        a: usize,
        b: usize,
        keys: &[u8],
        ca: f32,
        cb: f32,
    ) -> [f32; GAS_COUNT] {
        assert_ne!(a, b);
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        // new() checks CPU support. Vec and repr(align) preserve 32-byte alignment;
        // distinct checked indices guarantee disjoint mutable destinations.
        unsafe {
            transfer_avx2(self, archived, a, b, keys, ca, cb)
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        unreachable!("AVX2 is unavailable")
    }
}

pub fn avx2_available() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        false
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn transfer_avx2(
    current: &mut Simd,
    archived: &Simd,
    a: usize,
    b: usize,
    keys: &[u8],
    ca: f32,
    cb: f32,
) -> [f32; GAS_COUNT] {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;
    let mut mask = [0_i32; 24];
    for &gas in keys {
        mask[gas as usize] = -1;
    }
    // Validate indices before constructing pointers, including release builds.
    let aa = archived.0[a].moles.as_ptr();
    let ab = archived.0[b].moles.as_ptr();
    let pa = current.0[a].moles.as_mut_ptr();
    let pb = current.0[b].moles.as_mut_ptr();
    let mut deltas = [0.0; 24];
    let zero = _mm256_setzero_ps();
    let accuracy = _mm256_set1_ps(MOLAR_ACCURACY);
    for offset in [0, 8, 16] {
        let difference = _mm256_sub_ps(
            _mm256_load_ps(aa.add(offset)),
            _mm256_load_ps(ab.add(offset)),
        );
        // Match scalar operation boundaries; no reciprocal approximation or FMA contraction.
        let quantized = _mm256_mul_ps(
            _mm256_floor_ps(_mm256_add_ps(
                _mm256_div_ps(difference, accuracy),
                _mm256_set1_ps(0.5),
            )),
            accuracy,
        );
        let present = _mm256_castsi256_ps(_mm256_loadu_si256(mask.as_ptr().add(offset).cast()));
        let active = _mm256_and_ps(present, _mm256_cmp_ps::<_CMP_NEQ_OQ>(quantized, zero));
        let coefficient = _mm256_blendv_ps(
            _mm256_set1_ps(cb),
            _mm256_set1_ps(ca),
            _mm256_cmp_ps::<_CMP_GT_OQ>(quantized, zero),
        );
        let delta = _mm256_and_ps(_mm256_mul_ps(quantized, coefficient), active);
        let old_a = _mm256_load_ps(pa.add(offset));
        let old_b = _mm256_load_ps(pb.add(offset));
        // Skipped zero deltas must preserve signed zero and unused gas slots.
        _mm256_store_ps(
            pa.add(offset),
            _mm256_blendv_ps(old_a, _mm256_sub_ps(old_a, delta), active),
        );
        _mm256_store_ps(
            pb.add(offset),
            _mm256_blendv_ps(old_b, _mm256_add_ps(old_b, delta), active),
        );
        _mm256_storeu_ps(deltas.as_mut_ptr().add(offset), delta);
    }
    std::array::from_fn(|gas| deltas[gas])
}

#[derive(Clone)]
pub struct Soa {
    pub gases: [Vec<f32>; GAS_COUNT],
    pub temperatures: Vec<f32>,
}

impl Storage for Soa {
    fn new(count: usize) -> Self {
        Self {
            gases: std::array::from_fn(|_| vec![0.0; count]),
            temperatures: vec![0.0; count],
        }
    }
    #[inline(always)]
    fn get(&self, id: usize, gas: usize) -> f32 {
        self.gases[gas][id]
    }
    #[inline(always)]
    fn set(&mut self, id: usize, gas: usize, value: f32) {
        self.gases[gas][id] = value;
    }
    #[inline(always)]
    fn temperature(&self, id: usize) -> f32 {
        self.temperatures[id]
    }
    #[inline(always)]
    fn set_temperature(&mut self, id: usize, value: f32) {
        self.temperatures[id] = value;
    }
    fn bytes(&self) -> usize {
        (self.gases.iter().map(Vec::capacity).sum::<usize>() + self.temperatures.capacity()) * 4
    }
}
