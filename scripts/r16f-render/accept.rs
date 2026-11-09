// Dependency-free, allocation-free raw-byte acceptance. Not a GPU emulator.
#![no_std]
#![forbid(unsafe_code)]

pub const ROWS: usize = 16;
pub const WIDTH: usize = 3;
pub const HEIGHT: usize = ROWS + 2;
pub const PITCH: usize = 256;
pub const GUARD: usize = 256;
pub const RAW_BYTES: usize = 2 * GUARD + HEIGHT * PITCH;
pub const TRACE_BYTES: usize = 128 + ROWS * 32;
pub const READ_BYTES: usize = 128 + (WIDTH * HEIGHT + 8) * 48;
pub const POISON: u8 = 0xcd;
pub const TRACE_MAGIC: u32 = 0x5231_3600;
pub const READ_MAGIC: u32 = 0x5245_4100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Parameter,
    Guard,
    Trace,
    Preservation,
    Anchor,
    Readback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Seed,
    Clear,
    ClearZero,
    ClearNegativeZero,
    Load,
    Replace,
    Blend,
}

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub name: &'static str,
    pub kind: Kind,
    pub bits: u32,
    pub mask: u32,
    pub tag: u32,
    pub negative: bool,
}

// Only three render states and two mask-isolation controls, not a blend sweep.
pub const SPECS: [Spec; 14] = [
    Spec {
        name: "seed",
        kind: Kind::Seed,
        bits: 0,
        mask: 0,
        tag: 1,
        negative: false,
    },
    Spec {
        name: "clear",
        kind: Kind::Clear,
        bits: 0,
        mask: 0,
        tag: 2,
        negative: false,
    },
    Spec {
        name: "clear-zero",
        kind: Kind::ClearZero,
        bits: 0,
        mask: 0,
        tag: 3,
        negative: false,
    },
    Spec {
        name: "clear-negative-zero",
        kind: Kind::ClearNegativeZero,
        bits: 0,
        mask: 0,
        tag: 4,
        negative: false,
    },
    Spec {
        name: "load",
        kind: Kind::Load,
        bits: 0,
        mask: 0,
        tag: 5,
        negative: false,
    },
    Spec {
        name: "f32-replace",
        kind: Kind::Replace,
        bits: 32,
        mask: 15,
        tag: 6,
        negative: false,
    },
    Spec {
        name: "f16-replace",
        kind: Kind::Replace,
        bits: 16,
        mask: 15,
        tag: 7,
        negative: false,
    },
    Spec {
        name: "f32-none",
        kind: Kind::Replace,
        bits: 32,
        mask: 0,
        tag: 8,
        negative: false,
    },
    Spec {
        name: "f16-none",
        kind: Kind::Replace,
        bits: 16,
        mask: 0,
        tag: 9,
        negative: false,
    },
    Spec {
        name: "f32-blend",
        kind: Kind::Blend,
        bits: 32,
        mask: 15,
        tag: 10,
        negative: false,
    },
    Spec {
        name: "f16-blend",
        kind: Kind::Blend,
        bits: 16,
        mask: 15,
        tag: 11,
        negative: false,
    },
    Spec {
        name: "f32-red",
        kind: Kind::Replace,
        bits: 32,
        mask: 8,
        tag: 12,
        negative: false,
    },
    Spec {
        name: "f32-absent",
        kind: Kind::Replace,
        bits: 32,
        mask: 7,
        tag: 13,
        negative: false,
    },
    Spec {
        name: "negative-skip",
        kind: Kind::Replace,
        bits: 32,
        mask: 15,
        tag: 14,
        negative: true,
    },
];

// Below / at / above the half midpoint after 1.0, both signs; two finite-end
// ties and exact largest finite halves. No NaN, infinity or overflow campaign.
pub const SOURCE_R: [u32; ROWS] = [
    0x3f40_0000,
    0xc000_0000,
    0x3f80_0000,
    0x3e00_0000,
    0x0000_0000,
    0x8000_0000,
    0x3f80_0fff,
    0x3f80_1000,
    0x3f80_1001,
    0xbf80_0fff,
    0xbf80_1000,
    0xbf80_1001,
    0x477f_e000,
    0xc77f_e000,
    0x477f_d000,
    0xc77f_d000,
];
pub const DEST_R: [u16; ROWS] = [
    0x3400, 0xc400, 0x3e00, 0xb800, 0x8000, 0, 0x3400, 0x3400, 0x3400, 0x3400, 0x3400, 0x3400, 0,
    0, 0, 0,
];

pub fn source(row: usize) -> [u32; 4] {
    [
        SOURCE_R[row],
        0x3f00_0000,
        0xbe80_0000,
        [0x3e00_0000, 0x3f40_0000, 0, 0x3f80_0000][row % 4],
    ]
}
pub fn offset(x: usize, y: usize) -> usize {
    GUARD + y * PITCH + x * 2
}
pub fn word(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().expect("validated layout"))
}
pub fn short(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}
pub fn seed_half(x: usize, y: usize) -> u16 {
    if x == 1 && y > 0 && y <= ROWS {
        DEST_R[y - 1]
    } else {
        [0x3555, 0xb800, 0x7bff, 0x8000, 0, 0x3c00][(y * WIDTH + x) % 6]
    }
}
pub fn seed() -> [u8; RAW_BYTES] {
    let mut b = [POISON; RAW_BYTES];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let p = offset(x, y);
            b[p..p + 2].copy_from_slice(&seed_half(x, y).to_le_bytes());
        }
    }
    b
}

// Pure bit expansion: all 65536 half bit patterns, including signed zero.
pub const fn expand(h: u16) -> u32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let e = (h >> 10) & 31;
    let mut m = (h & 1023) as u32;
    if e == 0 {
        if m == 0 {
            return sign;
        }
        let mut exponent = 113;
        while m & 1024 == 0 {
            m <<= 1;
            exponent -= 1;
        }
        sign | (exponent << 23) | ((m & 1023) << 13)
    } else if e == 31 {
        sign | 0x7f80_0000 | (m << 13)
    } else {
        sign | (((e as u32) + 112) << 23) | (m << 13)
    }
}

// A bounded diagnostic model, NOT an asserted native rounding rule. Values
// outside this experiment's finite zero/normal domain are refused, not filled.
pub fn half_model(bits: u32, rne: bool) -> Option<u16> {
    let magnitude = bits & 0x7fff_ffff;
    let sign = ((bits >> 16) & 0x8000) as u16;
    if magnitude == 0 {
        return Some(sign);
    }
    if !(0x3880_0000..=0x477f_e000).contains(&magnitude) {
        return None;
    }
    let e = (magnitude >> 23) - 112;
    let q = (e << 10) | ((magnitude & 0x7f_ffff) >> 13);
    let r = magnitude & 8191;
    let up = rne && (r > 4096 || (r == 4096 && q & 1 != 0));
    Some(sign | (q + u32::from(up)) as u16)
}
pub fn exact_half(bits: u32) -> Option<u16> {
    let h = half_model(bits, false)?;
    (expand(h) == bits).then_some(h)
}

fn poison(b: &[u8]) -> Result<(), Error> {
    if b.iter().all(|&x| x == POISON) {
        Ok(())
    } else {
        Err(Error::Guard)
    }
}
pub fn guards(raw: &[u8], trace: &[u8], reads: &[u8]) -> Result<(), Error> {
    if raw.len() != RAW_BYTES || trace.len() != TRACE_BYTES || reads.len() != READ_BYTES {
        return Err(Error::Length);
    }
    poison(&raw[..GUARD])?;
    poison(&raw[GUARD + HEIGHT * PITCH..])?;
    for y in 0..HEIGHT {
        poison(&raw[GUARD + y * PITCH + WIDTH * 2..GUARD + (y + 1) * PITCH])?;
    }
    poison(&trace[..64])?;
    poison(&trace[64 + ROWS * 32..])?;
    poison(&reads[..64])?;
    poison(&reads[64 + WIDTH * HEIGHT * 48..])
}

pub fn canonical(raw: &[u8], reads: &[u8], tag: u32) -> Result<(), Error> {
    if raw.len() != RAW_BYTES || reads.len() != READ_BYTES {
        return Err(Error::Length);
    }
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let i = y * WIDTH + x;
            let p = 64 + i * 48;
            let h = short(raw, offset(x, y));
            let want = [
                expand(h),
                0,
                0,
                0x3f80_0000,
                u32::from(h),
                0,
                0,
                0x3c00,
                READ_MAGIC ^ tag,
                x as u32,
                y as u32,
                i as u32,
            ];
            for (lane, value) in want.into_iter().enumerate() {
                let actual = word(reads, p + lane * 4);
                // Observe zero signs separately in each read route; canonical
                // absent components remain exactly (G,B,A) = (0,0,1).
                let matches = if h & 0x7fff == 0 && lane == 0 {
                    actual & 0x7fff_ffff == 0
                } else if h & 0x7fff == 0 && lane == 4 {
                    actual <= 0xffff && actual & 0x7fff == 0
                } else {
                    actual == value
                };
                if !matches {
                    return Err(Error::Readback);
                }
            }
        }
    }
    Ok(())
}

// Check only algebraically exact, nonzero normal anchors for the blend. Ties,
// signed-zero arithmetic and rounding order remain measured observations.
pub fn blend_anchor(row: usize) -> Option<u16> {
    match row {
        0 => Some(0x3a80), // .75 + .25 * (1 - .75) = .8125
        1 => Some(0xcb00), // -2 + -4 * (1 - -2) = -14
        2 => Some(0x3c00), // 1 + 1.5 * (1 - 1) = 1
        3 => Some(0xb500), // .125 + -.5 * (1 - .125) = -.3125
        12 => Some(0x7bff),
        13 => Some(0xfbff),
        _ => None,
    }
}

// Ordinary validator: intentionally NO negative-control or skipped-row bypass.
pub fn validate(s: Spec, raw: &[u8], trace: &[u8], reads: &[u8]) -> Result<(), Error> {
    guards(raw, trace, reads)?;
    if ![0, 16, 32].contains(&s.bits) || s.mask > 15 {
        return Err(Error::Parameter);
    }
    if s.bits == 0 {
        poison(&trace[64..64 + ROWS * 32])?;
    } else {
        for row in 0..ROWS {
            let p = 64 + row * 32;
            if [
                word(trace, p),
                word(trace, p + 4),
                word(trace, p + 8),
                word(trace, p + 12),
            ] != [TRACE_MAGIC ^ s.tag, row as u32, s.bits, 0x7472_6163]
            {
                return Err(Error::Trace);
            }
            for (lane, input) in source(row).into_iter().enumerate() {
                let value = word(trace, p + 16 + lane * 4);
                if s.bits == 32 {
                    if value != input {
                        return Err(Error::Trace);
                    }
                } else {
                    if value > u16::MAX as u32 || value as u16 & 0x7c00 == 0x7c00 {
                        return Err(Error::Trace);
                    }
                    if let Some(h) = exact_half(input) {
                        // Sign of arithmetic zero is observed, not prescribed.
                        if h & 0x7fff == 0 {
                            if value & 0x7fff != 0 {
                                return Err(Error::Trace);
                            }
                        } else if value != u32::from(h) {
                            return Err(Error::Trace);
                        }
                    }
                }
            }
        }
    }
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let h = short(raw, offset(x, y));
            match s.kind {
                Kind::Seed | Kind::Load => {
                    if h != seed_half(x, y) {
                        return Err(Error::Preservation);
                    }
                }
                Kind::Clear => {
                    if h != 0x3600 {
                        return Err(Error::Anchor);
                    }
                }
                Kind::ClearZero | Kind::ClearNegativeZero => {
                    if h & 0x7fff != 0 {
                        return Err(Error::Anchor);
                    }
                }
                Kind::Replace | Kind::Blend => {
                    if x != 1 || y == 0 || y > ROWS || s.mask & 8 == 0 {
                        if h != seed_half(x, y) {
                            return Err(Error::Preservation);
                        }
                    } else {
                        let row = y - 1;
                        if s.kind == Kind::Blend {
                            if blend_anchor(row).is_some_and(|want| h != want) {
                                return Err(Error::Anchor);
                            }
                        } else if s.bits == 16 {
                            let value = word(trace, 64 + row * 32 + 16) as u16;
                            if value & 0x7fff == 0 {
                                if h & 0x7fff != 0 {
                                    return Err(Error::Anchor);
                                }
                            } else if h != value {
                                return Err(Error::Anchor);
                            }
                        } else if let Some(want) = exact_half(SOURCE_R[row]) {
                            if want & 0x7fff == 0 {
                                if h & 0x7fff != 0 {
                                    return Err(Error::Anchor);
                                }
                            } else if h != want {
                                return Err(Error::Anchor);
                            }
                        }
                    }
                }
            }
            // All selected inputs/results are finite within this bounded suite.
            if h & 0x7c00 == 0x7c00 {
                return Err(Error::Anchor);
            }
        }
    }
    canonical(raw, reads, s.tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put(b: &mut [u8], p: usize, v: u32) {
        b[p..p + 4].copy_from_slice(&v.to_le_bytes());
    }
    // SOFTWARE-ONLY fixtures exist only in --test binaries, never the capture CLI.
    fn fixture(s: Spec) -> ([u8; RAW_BYTES], [u8; TRACE_BYTES], [u8; READ_BYTES]) {
        let (mut raw, mut trace, mut reads) = (seed(), [POISON; TRACE_BYTES], [POISON; READ_BYTES]);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let h = match s.kind {
                    Kind::Clear => 0x3600,
                    Kind::ClearZero => 0,
                    Kind::ClearNegativeZero => 0x8000,
                    Kind::Replace | Kind::Blend
                        if x == 1 && y > 0 && y <= ROWS && s.mask & 8 != 0 =>
                    {
                        let i = y - 1;
                        if s.kind == Kind::Blend {
                            blend_anchor(i).unwrap_or(0)
                        } else {
                            half_model(SOURCE_R[i], true).unwrap()
                        }
                    }
                    _ => seed_half(x, y),
                };
                let p = offset(x, y);
                raw[p..p + 2].copy_from_slice(&h.to_le_bytes());
                let i = y * WIDTH + x;
                let p = 64 + i * 48;
                for (k, v) in [
                    expand(h),
                    0,
                    0,
                    0x3f80_0000,
                    u32::from(h),
                    0,
                    0,
                    0x3c00,
                    READ_MAGIC ^ s.tag,
                    x as u32,
                    y as u32,
                    i as u32,
                ]
                .into_iter()
                .enumerate()
                {
                    put(&mut reads, p + k * 4, v);
                }
            }
        }
        if s.bits != 0 {
            for row in 0..ROWS {
                let p = 64 + row * 32;
                for (k, v) in [TRACE_MAGIC ^ s.tag, row as u32, s.bits, 0x7472_6163]
                    .into_iter()
                    .enumerate()
                {
                    put(&mut trace, p + k * 4, v);
                }
                for (k, v) in source(row).into_iter().enumerate() {
                    put(
                        &mut trace,
                        p + 16 + k * 4,
                        if s.bits == 32 {
                            v
                        } else {
                            u32::from(half_model(v, true).unwrap())
                        },
                    );
                }
            }
        }
        (raw, trace, reads)
    }
    #[test]
    fn half_expansion_and_model_bounds() {
        for (h, f) in [
            (0, 0),
            (0x8000, 0x8000_0000),
            (1, 0x3380_0000),
            (0x0400, 0x3880_0000),
            (0x3c00, 0x3f80_0000),
            (0x7bff, 0x477f_e000),
        ] {
            assert_eq!(expand(h), f);
        }
        for h in 0x0400..=0x7bff {
            assert_eq!(exact_half(expand(h)), Some(h));
        }
        for h in 0x8400..=0xfbff {
            assert_eq!(exact_half(expand(h)), Some(h));
        }
        for bits in [1, 0x7f80_0000, 0x7fc0_0000, 0x4780_0000] {
            assert_eq!(half_model(bits, true), None);
        }
        assert_eq!(half_model(0x3f80_1000, true), Some(0x3c00));
        assert_eq!(half_model(0x3f80_3000, true), Some(0x3c02));
        assert_eq!(half_model(0x3f80_1001, false), Some(0x3c00));
        assert_eq!(half_model(0x3f80_1001, true), Some(0x3c01));
    }
    #[test]
    fn all_layouts_and_truncations() {
        for s in SPECS {
            let (a, b, c) = fixture(s);
            assert_eq!(validate(s, &a, &b, &c), Ok(()));
            for n in 0..a.len() {
                assert_eq!(validate(s, &a[..n], &b, &c), Err(Error::Length));
            }
            for n in 0..b.len() {
                assert_eq!(validate(s, &a, &b[..n], &c), Err(Error::Length));
            }
            for n in 0..c.len() {
                assert_eq!(validate(s, &a, &b, &c[..n]), Err(Error::Length));
            }
        }
    }
    #[test]
    fn every_seed_byte_and_read_lane_is_checked() {
        let s = SPECS[0];
        let (a, b, c) = fixture(s);
        for i in 0..a.len() {
            let mut v = a;
            v[i] ^= 1;
            assert!(validate(s, &v, &b, &c).is_err());
        }
        for i in 0..b.len() {
            let mut v = b;
            v[i] ^= 1;
            assert!(validate(s, &a, &v, &c).is_err());
        }
        for i in 0..c.len() {
            let mut v = c;
            v[i] ^= 1;
            assert!(validate(s, &a, &b, &v).is_err());
        }
    }
    #[test]
    fn skip_negative_has_no_validator_exception() {
        let s = SPECS[13];
        let (a, mut b, c) = fixture(s);
        b[64..96].fill(POISON);
        assert_eq!(validate(s, &a, &b, &c), Err(Error::Trace));
    }
    #[test]
    fn replacement_does_not_qualify_blending() {
        let s = SPECS[5];
        let (a, b, c) = fixture(s);
        assert_eq!(
            validate(
                Spec {
                    kind: Kind::Blend,
                    ..s
                },
                &a,
                &b,
                &c
            ),
            Err(Error::Anchor)
        );
        let s = SPECS[9];
        let (mut a, b, c) = fixture(s);
        // Wrong one-minus-source-alpha gives .96875 at row zero, not .8125.
        let p = offset(1, 1);
        a[p..p + 2].copy_from_slice(&0x3bc0u16.to_le_bytes());
        assert_eq!(validate(s, &a, &b, &c), Err(Error::Anchor));
    }
    #[test]
    fn masks_and_clear_are_independent_of_trace() {
        for i in [7, 8, 12] {
            let s = SPECS[i];
            let (mut a, b, c) = fixture(s);
            a[offset(1, 1)] ^= 1;
            assert_eq!(validate(s, &a, &b, &c), Err(Error::Preservation));
        }
        let s = SPECS[1];
        let (mut a, b, c) = fixture(s);
        a[offset(0, 0)] ^= 1;
        assert_eq!(validate(s, &a, &b, &c), Err(Error::Anchor));
    }
    #[test]
    fn observations_do_not_impose_a_signed_zero_rule() {
        for i in [2, 3] {
            let s = SPECS[i];
            let (a, b, c) = fixture(Spec {
                kind: Kind::ClearZero,
                ..s
            });
            assert_eq!(validate(s, &a, &b, &c), Ok(()));
        }
        let s = SPECS[3];
        let (a, b, mut c) = fixture(s);
        for i in 0..WIDTH * HEIGHT {
            put(&mut c, 64 + i * 48, 0);
            put(&mut c, 64 + i * 48 + 16, 0);
        }
        assert_eq!(validate(s, &a, &b, &c), Ok(()));
    }
}
