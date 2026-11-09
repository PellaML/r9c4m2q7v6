// Allocation-free, dependency-free acceptance core; not an emulator change.
#![no_std]
#![forbid(unsafe_code)]

pub const MODES: [&str; 2] = ["precise", "fast"];
pub const CASES: [(u16, u16); 20] = [
    (0x0000, 0), (0x8000, 0), // signed zeros
    (0x0001, 0), (0x8001, 0), (0x0002, 0), (0x8002, 0), // tiny subnormals
    (0x03ff, 0), (0x83ff, 0), (0x0400, 0), (0x8400, 0), // subnormal/normal boundary
    (0x3800, 0), (0xb800, 0), (0x3bff, 0), (0xbbff, 0), // +/-0.5, +/-near1
    (0x3c00, 1), (0x3e00, 1), (0x4100, 2), // 1, 1.5, 2.5: RTZ, not nearest
    (0x77ff, 0x7ff0), (0x7800, 0x8000), (0x7bff, 0xffe0), // unsigned high end
];
pub const BASE: f32 = 153600.25; // 150 * 1024 + 1/4, independently selected.
pub const CONVERSION_BYTES: usize = 512; // 64 + 32*12 + 64; only 20 rows execute.
pub const IMAGE_BYTES: usize = 1536; // 256 + four 256-byte rows + 256.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error { Length, Parameter, Guard, Stamp, Input, Integer, NonFinite, HalfRange, Interpolation }

pub fn word(bytes: &[u8], offset: usize) -> u32 {
    // Used only after an exact-length check by the parsers below.
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("checked bounds"))
}
fn short(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn poison(bytes: &[u8]) -> Result<(), Error> {
    if bytes.iter().all(|&b| b == 0xcd) { Ok(()) } else { Err(Error::Guard) }
}

// Independent integer-field RTZ oracle. Invalid domains stay None, not saturated.
pub const fn rtz_half(h: u16) -> Option<u16> {
    let magnitude = h & 0x7fff;
    if magnitude >= 0x7c00 || (h & 0x8000 != 0 && magnitude >= 0x3c00) { return None; }
    let exponent = (magnitude >> 10) as u32;
    if exponent < 15 { return Some(0); }
    let significand = 1024u32 + (magnitude & 1023) as u32;
    Some(if exponent < 25 { (significand >> (25 - exponent)) as u16 }
         else { (significand << (exponent - 25)) as u16 })
}

pub fn conversion(bytes: &[u8], mode: u32) -> Result<(), Error> {
    if bytes.len() != CONVERSION_BYTES { return Err(Error::Length); }
    if mode > 1 { return Err(Error::Parameter); }
    poison(&bytes[..64])?;
    poison(&bytes[64 + CASES.len() * 12..])?; // Includes over-dispatch and suffix guards.
    for (i, &(input, expected)) in CASES.iter().enumerate() {
        let p = 64 + i * 12;
        if short(bytes, p) != expected { return Err(Error::Integer); }
        poison(&bytes[p + 2..p + 4])?;
        if short(bytes, p + 4) != input { return Err(Error::Input); }
        poison(&bytes[p + 6..p + 8])?;
        if word(bytes, p + 8) != 0x4d310000 | (mode << 8) | i as u32 { return Err(Error::Stamp); }
    }
    Ok(())
}

// Screen triangle (0,0), (8,0), (0,8), viewport 4x4, one center sample.
// BOTH cases use unequal w = [1,2,4]. Case 0 is constant; case 1 is nonconstant.
// Both links in that ONE control measure perspective versus affine interpolation.
// These are ideal mathematical reference values, NOT asserted hardware rounding.
pub fn ideal(case: usize, pixel: usize) -> [u32; 6] {
    let l1 = ((pixel % 4) as f64 + 0.5) / 8.0;
    let l2 = ((pixel / 4) as f64 + 0.5) / 8.0;
    let denominator = 1.0 - l1 - l2 + l1 / 2.0 + l2 / 4.0;
    let (p1, p2) = (l1 / 2.0 / denominator, l2 / 4.0 / denominator);
    let value = |a: f64, b: f64| if case == 0 { BASE as f64 }
        else { BASE as f64 + 1024.0 * a - 512.0 * b };
    [value(p1, p2), value(l1, l2), p1, p2, l1, l2].map(|x| (x as f32).to_bits())
}

pub fn observed(a: &[u8], b: &[u8], pixel: usize) -> [u32; 6] {
    let p = 256 + pixel / 4 * 256 + pixel % 4 * 16;
    [word(a, p), word(a, p + 4), word(a, p + 8), word(a, p + 12), word(b, p), word(b, p + 4)]
}
pub fn measured_weights_model(case: usize, v: [u32; 6]) -> u32 {
    let value = if case == 0 { BASE as f64 } else {
        BASE as f64 + 1024.0 * f32::from_bits(v[2]) as f64 - 512.0 * f32::from_bits(v[3]) as f64
    };
    (value as f32).to_bits()
}
fn ulps(actual: u32, expected: u32) -> Result<u32, Error> {
    if !f32::from_bits(actual).is_finite() || actual >> 31 != 0 { return Err(Error::NonFinite); }
    Ok(actual.abs_diff(expected)) // All reference values are strictly positive.
}
fn image_guards(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() != IMAGE_BYTES { return Err(Error::Length); }
    poison(&bytes[..256])?;
    for row in 0..4 { poison(&bytes[256 + row * 256 + 64..256 + (row + 1) * 256])?; }
    poison(&bytes[1280..])
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics { pub max_ulps: [u32; 6], pub exact_counts: [u32; 6], pub measured_model_max_ulps: u32 }

pub fn varying(a: &[u8], b: &[u8], mode: u32, case: usize) -> Result<Metrics, Error> {
    if mode > 1 || case > 1 { return Err(Error::Parameter); }
    image_guards(a)?; image_guards(b)?;
    let mut metrics = Metrics { max_ulps: [0; 6], exact_counts: [0; 6], measured_model_max_ulps: 0 };
    for pixel in 0..16 {
        let p = 256 + pixel / 4 * 256 + pixel % 4 * 16;
        let xy = (pixel % 4) as u32 | ((pixel / 4) as u32) << 16;
        let stamp = 0x4d320000 | (mode << 8) | ((case as u32) << 4) | pixel as u32;
        if word(b, p + 8) != xy || word(b, p + 12) != stamp { return Err(Error::Stamp); }
        let values = observed(a, b, pixel);
        let expected = ideal(case, pixel);
        for channel in 0..6 {
            let delta = ulps(values[channel], expected[channel])?;
            if channel < 2 && f32::from_bits(values[channel]) <= 65504.0 { return Err(Error::HalfRange); }
            // Constant preservation and affine basis must be BIT EXACT. The one
            // nonconstant control and perspective basis get a <=4-ULP sanity
            // envelope, NOT an asserted universal interpolation rounding law.
            let tolerance = if channel >= 4 || (case == 0 && channel < 2) { 0 } else { 4 };
            if delta > tolerance { return Err(Error::Interpolation); }
            metrics.max_ulps[channel] = metrics.max_ulps[channel].max(delta);
            metrics.exact_counts[channel] += u32::from(delta == 0);
        }
        let delta = ulps(values[0], measured_weights_model(case, values))?;
        if delta > 4 { return Err(Error::Interpolation); }
        metrics.measured_model_max_ulps = metrics.measured_model_max_ulps.max(delta);
    }
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put(bytes: &mut [u8], p: usize, value: u32) { bytes[p..p + 4].copy_from_slice(&value.to_le_bytes()); }
    fn integer_fixture(mode: u32) -> [u8; CONVERSION_BYTES] {
        let mut bytes = [0xcd; CONVERSION_BYTES];
        for (i, &(h, value)) in CASES.iter().enumerate() {
            let p = 64 + i * 12;
            bytes[p..p + 2].copy_from_slice(&value.to_le_bytes());
            bytes[p + 4..p + 6].copy_from_slice(&h.to_le_bytes());
            put(&mut bytes, p + 8, 0x4d310000 | (mode << 8) | i as u32);
        }
        bytes
    }
    fn image_fixture(case: usize) -> ([u8; IMAGE_BYTES], [u8; IMAGE_BYTES]) {
        let (mut a, mut b) = ([0xcd; IMAGE_BYTES], [0xcd; IMAGE_BYTES]);
        for i in 0..16 {
            let p = 256 + i / 4 * 256 + i % 4 * 16;
            let v = ideal(case, i);
            for c in 0..4 { put(&mut a, p + c * 4, v[c]); }
            put(&mut b, p, v[4]); put(&mut b, p + 4, v[5]);
            put(&mut b, p + 8, (i % 4) as u32 | ((i / 4) as u32) << 16);
            put(&mut b, p + 12, 0x4d320000 | ((case as u32) << 4) | i as u32);
        }
        (a, b)
    }
    #[test] fn exact_table_and_refused_domains() {
        for &(h, expected) in &CASES { assert_eq!(rtz_half(h), Some(expected)); }
        for h in [0xbc00, 0xfbff, 0x7c00, 0xfc00, 0x7e00, 0xfe00] { assert_eq!(rtz_half(h), None); }
        let admitted = (0..=u16::MAX).filter(|&h| rtz_half(h).is_some()).count();
        assert_eq!(admitted, 47104); // Software-domain check only; not native coverage.
    }
    #[test] fn conversion_exact_and_all_truncations() {
        for mode in 0..2 {
            let b = integer_fixture(mode); assert_eq!(conversion(&b, mode), Ok(()));
            for n in 0..b.len() { assert_eq!(conversion(&b[..n], mode), Err(Error::Length)); }
        }
    }
    #[test] fn conversion_rejects_every_byte_mutation() {
        let good = integer_fixture(0);
        for i in 0..good.len() { let mut b = good; b[i] ^= 1; assert!(conversion(&b, 0).is_err()); }
        assert_eq!(conversion(&good, 1), Err(Error::Stamp));
    }
    #[test] fn varying_models_and_truncations() {
        for case in 0..2 {
            let (a, b) = image_fixture(case); assert!(varying(&a, &b, 0, case).is_ok());
            for n in 0..a.len() { assert!(varying(&a[..n], &b, 0, case).is_err()); }
        }
    }
    #[test] fn rejects_half_flat_affine_substitution_and_bad_guards() {
        let (a, b) = image_fixture(1);
        for bad in [65504.0f32.to_bits(), f32::INFINITY.to_bits(), f32::NAN.to_bits(), BASE.to_bits(), ideal(1, 0)[1]] {
            let mut changed = a; put(&mut changed, 256, bad);
            assert!(varying(&changed, &b, 0, 1).is_err());
        }
        for offset in [0, 255, 320, 511, 1280, 1535] {
            let mut changed = a; changed[offset] ^= 1;
            assert_eq!(varying(&changed, &b, 0, 1), Err(Error::Guard));
        }
        assert_eq!(varying(&a, &b, 1, 1), Err(Error::Stamp));
    }
    #[test] fn unequal_w_constant_must_be_bit_exact() {
        let (a, b) = image_fixture(0);
        assert_ne!(ideal(0, 0)[2], ideal(0, 0)[4]); // Distinguishable unequal W.
        for channel in 0..2 {
            let mut changed = a;
            put(&mut changed, 256 + channel * 4, BASE.to_bits() + 1);
            assert_eq!(varying(&changed, &b, 0, 0), Err(Error::Interpolation));
        }
        for n in 0..b.len() { assert!(varying(&a, &b[..n], 0, 0).is_err()); }
        let mut changed = b; put(&mut changed, 264, 1);
        assert_eq!(varying(&a, &changed, 0, 0), Err(Error::Stamp));
    }
}
