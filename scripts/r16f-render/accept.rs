// Dependency-free, allocation-free raw-byte acceptance. Not a GPU emulator.
#![no_std]
#![forbid(unsafe_code)]

pub const ROWS: usize = 64;
pub const MATH_STRIDE: usize = 96;
pub const MATH_BYTES: usize = 128 + (ROWS + 4) * MATH_STRIDE;
pub const MODES: [&str; 2] = ["precise", "fast"];
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
    ClearProbe,
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
pub const SPECS: [Spec; 21] = [
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
    Spec {
        name: "clear-tie",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 15,
        negative: false,
    },
    Spec {
        name: "clear-tie-below",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 16,
        negative: false,
    },
    Spec {
        name: "clear-tie-above",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 17,
        negative: false,
    },
    Spec {
        name: "clear-subnormal",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 18,
        negative: false,
    },
    Spec {
        name: "clear-underflow",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 19,
        negative: false,
    },
    Spec {
        name: "clear-overflow",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 20,
        negative: false,
    },
    Spec {
        name: "clear-odd-tie",
        kind: Kind::ClearProbe,
        bits: 0,
        mask: 0,
        tag: 21,
        negative: false,
    },
];

// Frozen synthetic discriminators. No captured values or source clamping.
pub const CASES: [(&str, u32, u16); ROWS] = [
    ("normal-anchor", 0x3f400000, 0x3400),
    ("negative-no-clamp", 0xc0000000, 0xc400),
    ("normal-anchor", 0x3f800000, 0x3e00),
    ("normal-anchor", 0x3e000000, 0xb800),
    ("zero-plus-minus", 0x00000000, 0x8000),
    ("zero-minus-plus", 0x80000000, 0x0000),
    ("tie-below", 0x3f800fff, 0x3400),
    ("tie-even", 0x3f801000, 0x3400),
    ("tie-above", 0x3f801001, 0x3400),
    ("negative-tie-below", 0xbf800fff, 0x3400),
    ("negative-tie", 0xbf801000, 0x3400),
    ("negative-tie-above", 0xbf801001, 0x3400),
    ("finite-max", 0x477fe000, 0x0000),
    ("negative-finite-max", 0xc77fe000, 0x0000),
    ("finite-end-tie", 0x477fd000, 0x0000),
    ("negative-finite-end-tie", 0xc77fd000, 0x0000),
    ("zero-plus-plus", 0x00000000, 0x0000),
    ("zero-minus-minus", 0x80000000, 0x8000),
    ("f32-min-subnormal", 0x00000001, 0x0000),
    ("negative-f32-min-subnormal", 0x80000001, 0x0000),
    ("half-min-subnormal", 0x33800000, 0x0000),
    ("negative-half-min-subnormal", 0xb3800000, 0x0000),
    ("half-underflow-tie", 0x33000000, 0x0000),
    ("negative-half-underflow-tie", 0xb3000000, 0x0000),
    ("half-underflow-above", 0x33000001, 0x0000),
    ("negative-half-underflow-above", 0xb3000001, 0x0000),
    ("half-max-subnormal", 0x387fc000, 0x0000),
    ("negative-half-max-subnormal", 0xb87fc000, 0x0000),
    ("destination-min-subnormal", 0x00000000, 0x0001),
    ("negative-destination-min-subnormal", 0x80000000, 0x8001),
    ("half-min-normal", 0x38800000, 0x0000),
    ("negative-half-min-normal", 0xb8800000, 0x0000),
    ("source-overflow-below", 0x477fefff, 0x0000),
    ("source-overflow-tie", 0x477ff000, 0x0000),
    ("half-fma-owned-c", 0x3ec32000, 0x3afb),
    ("negative-source-overflow-tie", 0xc77ff000, 0x0000),
    ("union-positive-overflow", 0xc3000000, 0x6000),
    ("union-negative-overflow", 0x43020000, 0x6000),
    ("cancellation-zero", 0x40000000, 0x4000),
    ("cancellation-below", 0x3fffffff, 0x4000),
    ("cancellation-above", 0x40000001, 0x4000),
    ("positive-no-clamp", 0x40000000, 0x3400),
    ("fma-f32-subnormal-witness", 0xbeffe7df, 0x3555),
    ("fma-half-normal-witness-a", 0x3b226000, 0x9978),
    ("fma-half-normal-witness-b", 0x3b86e000, 0x9c25),
    ("fma-half-large-witness", 0x46e52000, 0x3b91),
    ("half-fma-owned-a", 0x3e44c000, 0x391b),
    ("half-fma-owned-b", 0x3e3c4000, 0x37ed),
    ("normal-cancellation-underflow", 0x3b000000, 0x9800),
    ("normal-negative-cancellation-underflow", 0xbb000000, 0x1800),
    ("normal-cancellation-min-half", 0x39800000, 0x8c00),
    ("normal-cancellation-below-half", 0x39000000, 0x8800),
    ("rounded-half-source-0", 0xbe00a000, 0x3fb9),
    ("rounded-half-source-1", 0xc07a2000, 0xbe27),
    ("rounded-half-source-2", 0xc06fa000, 0x3dd3),
    ("rounded-half-source-3", 0xbdd26000, 0x349d),
    ("rounded-half-source-4", 0x3dfee000, 0xb7a5),
    ("rounded-half-source-5", 0xbdac6000, 0x3799),
    ("f32-rtz-order-witness-0", 0x3d4b89b4, 0x26e3),
    ("f32-rtz-order-witness-1", 0x3e960f09, 0xb6a1),
    ("f32-rtz-order-witness-2", 0xbcad11d8, 0x3ae4),
    ("f32-rtz-order-witness-3", 0x3b3f8f9a, 0xa3db),
    ("f32-rtz-order-witness-4", 0xbc8b9956, 0xa6b2),
    ("f32-rtz-order-witness-5", 0xbe850549, 0x329a),
];
const fn sources() -> [u32; ROWS] {
    let mut a = [0; ROWS];
    let mut i = 0;
    while i < ROWS {
        a[i] = CASES[i].1;
        i += 1;
    }
    a
}
const fn destinations() -> [u16; ROWS] {
    let mut a = [0; ROWS];
    let mut i = 0;
    while i < ROWS {
        a[i] = CASES[i].2;
        i += 1;
    }
    a
}
pub fn domain(row: usize) -> &'static str {
    match row {
        0..=3 | 6..=11 | 34 | 38..=63 => "normal-input-core",
        32..=37 => "overflow-observation",
        4..=5 | 16..=17 => "zero-observation",
        12..=15 => "finite-end-observation",
        _ => "subnormal-observation",
    }
}
pub const SOURCE_R: [u32; ROWS] = sources();
pub const DEST_R: [u16; ROWS] = destinations();
pub fn row_enabled(tag: u32, row: usize) -> bool {
    !(tag & 256 != 0 && (32..38).contains(&row) && row != 34)
}
pub fn capture_enabled(s: Spec, mode: u32) -> bool {
    !(mode == 1 && s.name == "clear-overflow")
}
pub fn mode_spec(mut s: Spec, mode: u32) -> Spec {
    s.tag |= mode << 8;
    s
}
pub const fn math_tag(mode: u32, bits: u32) -> u32 {
    0x6000 | (mode << 8) | bits
}
pub fn clear_value(s: Spec) -> u64 {
    match s.kind {
        Kind::Clear => 0x3fd8_0000_0000_0000,
        Kind::ClearNegativeZero => 0x8000_0000_0000_0000,
        Kind::ClearProbe => match s.tag & 255 {
            15 => 0x3ff0_0200_0000_0000,
            16 => 0x3ff0_01ff_ffff_ffff,
            17 => 0x3ff0_0200_0000_0001,
            18 => 0x3e70_0000_0000_0000,
            19 => 0x3e60_0000_0000_0000,
            20 => 0x40ef_fe00_0000_0000,
            21 => 0x3ff0_0600_0000_0000,
            _ => 0,
        },
        _ => 0,
    }
}
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

// IEEE integer-field hypotheses ONLY. NaN payloads are not predicted.
fn narrow(sign: u16, exponent: i32, mantissa: u64, fraction_bits: u32, rne: bool) -> u16 {
    if exponent > 15 {
        return sign | if rne { 0x7c00 } else { 0x7bff };
    }
    if exponent < -25 {
        return sign;
    }
    let (q, r, shift) = if exponent >= -14 {
        let sh = fraction_bits - 10;
        (
            (((exponent + 15) as u64) << 10) | (mantissa >> sh),
            mantissa & ((1u64 << sh) - 1),
            sh,
        )
    } else {
        let sh = (fraction_bits as i32 - 24 - exponent) as u32;
        let v = mantissa | (1u64 << fraction_bits);
        (v >> sh, v & ((1u64 << sh) - 1), sh)
    };
    let midpoint = 1u64 << (shift - 1);
    sign | (q + u64::from(rne && (r > midpoint || (r == midpoint && q & 1 != 0)))) as u16
}
pub fn half_model(bits: u32, rne: bool) -> Option<u16> {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let e = (bits >> 23) & 255;
    let m = bits & 0x7f_ffff;
    if e == 255 {
        return if m == 0 { Some(sign | 0x7c00) } else { None };
    }
    Some(narrow(sign, e as i32 - 127, u64::from(m), 23, rne))
}
pub fn half64_model(bits: u64, rne: bool) -> Option<u16> {
    let sign = ((bits >> 48) & 0x8000) as u16;
    let e = (bits >> 52) & 2047;
    let m = bits & 0x000f_ffff_ffff_ffff;
    if e == 2047 {
        return if m == 0 { Some(sign | 0x7c00) } else { None };
    }
    Some(narrow(sign, e as i32 - 1023, m, 52, rne))
}
// Exact finite half FMA hypothesis: integer multiples of 2^-48, one final
// half rounding. No f32 double rounding and no host floating-point dependency.
pub fn half_fma(s: u16, d: u16, k: u16, rne: bool) -> Option<u16> {
    if [s, d, k].iter().any(|h| h & 0x7c00 == 0x7c00) {
        return None;
    }
    fn scaled(h: u16) -> i128 {
        let e = (h >> 10) & 31;
        let m = i128::from(h & 1023);
        let v = if e == 0 { m } else { (1024 + m) << (e - 1) };
        if h & 0x8000 != 0 {
            -v
        } else {
            v
        }
    }
    let value = (scaled(s) << 24) + scaled(d) * scaled(k);
    if value == 0 {
        return Some(
            if s & 0x7fff == 0
                && (d & 0x7fff == 0 || k & 0x7fff == 0)
                && s & 0x8000 != 0
                && (d ^ k) & 0x8000 != 0
            {
                0x8000
            } else {
                0
            },
        );
    }
    let sign = if value < 0 { 0x8000 } else { 0 };
    let n = if value < 0 {
        (-value) as u128
    } else {
        value as u128
    };
    let exponent = (127 - n.leading_zeros()) as i32 - 48;
    if exponent > 15 {
        return Some(sign | if rne { 0x7c00 } else { 0x7bff });
    }
    if exponent < -25 {
        return Some(sign);
    }
    let shift = if exponent >= -14 {
        (38 + exponent) as u32
    } else {
        24
    };
    let q = if exponent >= -14 {
        (((exponent + 15) as u128) << 10) + ((n >> shift) - 1024)
    } else {
        n >> shift
    };
    let rem = n & ((1u128 << shift) - 1);
    let mid = 1u128 << (shift - 1);
    Some(sign | (q + u128::from(rne && (rem > mid || (rem == mid && q & 1 != 0)))) as u16)
}
pub fn normal(h: u16) -> bool {
    h & 0x7c00 != 0 && h & 0x7c00 != 0x7c00
}
pub fn math_capture(bytes: &[u8], mode: u32, bits: u32) -> Result<(), Error> {
    if bytes.len() != MATH_BYTES {
        return Err(Error::Length);
    }
    if mode > 1 || ![16, 32].contains(&bits) {
        return Err(Error::Parameter);
    }
    poison(&bytes[..64])?;
    poison(&bytes[64 + ROWS * MATH_STRIDE..])?;
    for i in 0..ROWS {
        let p = 64 + i * MATH_STRIDE;
        if !row_enabled(mode << 8, i) {
            poison(&bytes[p..p + MATH_STRIDE])?;
            continue;
        }
        if word(bytes, p) != 0x554e_4900 ^ math_tag(mode, bits)
            || word(bytes, p + 4) != i as u32
            || word(bytes, p + 8) != bits
            || word(bytes, p + 16) != u32::from(DEST_R[i])
            || word(bytes, p + 60) != SOURCE_R[i]
        {
            return Err(Error::Trace);
        }
        if word(bytes, p + 88) != 0x6861_6c66 || word(bytes, p + 92) != u32::from(DEST_R[i]) {
            return Err(Error::Trace);
        }
        for lane in [12, 13, 14, 16, 17, 18, 19] {
            if word(bytes, p + lane * 4) > 0xffff {
                return Err(Error::Trace);
            }
        }
    }
    Ok(())
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
                let matches = if h & 0x7fff != 0 && !normal(h) && (lane == 0 || lane == 4) {
                    lane != 4 || actual <= 0xffff
                } else if h & 0x7fff == 0 && lane == 0 {
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
        0 => Some(0x3a80),  // .75 + .25 * (1 - .75) = .8125
        1 => Some(0xcb00),  // -2 + -4 * (1 - -2) = -14
        2 => Some(0x3c00),  // 1 + 1.5 * (1 - 1) = 1
        3 => Some(0xb500),  // .125 + -.5 * (1 - .125) = -.3125
        41 => Some(0x3f00), // 2 + .25 * (1 - 2) = 1.75; never clamp source.

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
            if !row_enabled(s.tag, row) {
                poison(&trace[p..p + 32])?;
                continue;
            }
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
                    if value > u16::MAX as u32 {
                        return Err(Error::Trace);
                    }
                    if input & 0x7fff_ffff == 0 && value & 0x7fff != 0 {
                        return Err(Error::Trace);
                    }
                    if let Some(h) = exact_half(input).filter(|h| normal(*h)) {
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
                    if seed_half(x, y) & 0x7fff == 0 && h & 0x7fff != 0 {
                        return Err(Error::Preservation);
                    }
                    if h != seed_half(x, y) && (s.kind == Kind::Seed || normal(seed_half(x, y))) {
                        return Err(Error::Preservation);
                    }
                }
                Kind::Clear => {
                    if h != 0x3600 {
                        return Err(Error::Anchor);
                    }
                }
                Kind::ClearProbe => {
                    if h != short(raw, offset(0, 0)) {
                        return Err(Error::Anchor);
                    }
                }
                Kind::ClearZero | Kind::ClearNegativeZero => {
                    if h & 0x7fff != 0 {
                        return Err(Error::Anchor);
                    }
                }
                Kind::Replace | Kind::Blend => {
                    if x != 1
                        || y == 0
                        || y > ROWS
                        || s.mask & 8 == 0
                        || !row_enabled(s.tag, y.saturating_sub(1))
                    {
                        if seed_half(x, y) & 0x7fff == 0 && h & 0x7fff != 0 {
                            return Err(Error::Preservation);
                        }
                        if normal(seed_half(x, y)) && h != seed_half(x, y) {
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
                            } else if normal(value) && h != value {
                                return Err(Error::Anchor);
                            }
                        } else if let Some(want) = exact_half(SOURCE_R[row]).filter(|h| normal(*h))
                        {
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
                    Kind::ClearProbe => half64_model(clear_value(s), true).unwrap(),
                    Kind::ClearZero => 0,
                    Kind::ClearNegativeZero => 0x8000,
                    Kind::Replace | Kind::Blend
                        if x == 1
                            && y > 0
                            && y <= ROWS
                            && s.mask & 8 != 0
                            && row_enabled(s.tag, y - 1) =>
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
                if !row_enabled(s.tag, row) {
                    continue;
                }
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
        assert_eq!(half_model(0x7fc0_0000, true), None);
        for (b, rne, rtz) in [
            (1, 0, 0),
            (0x33800000, 1, 1),
            (0x33000000, 0, 0),
            (0x33000001, 1, 0),
            (0x477ff000, 0x7c00, 0x7bff),
            (0xc77ff000, 0xfc00, 0xfbff),
            (0x7f800000, 0x7c00, 0x7c00),
        ] {
            assert_eq!(half_model(b, true), Some(rne));
            assert_eq!(half_model(b, false), Some(rtz));
        }
        for h in 0u16..=0xffff {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(half_model(expand(h), true), Some(h));
            }
        }
        for s in &SPECS[14..] {
            assert_eq!(half64_model(clear_value(*s), true).is_some(), true);
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
            let active = i >= 64 && i < 64 + WIDTH * HEIGHT * 48;
            let lane = if active { (i - 64) % 48 / 4 } else { 99 };
            let pixel = if active { (i - 64) / 48 } else { 0 };
            if !active
                || normal(seed_half(pixel % WIDTH, pixel / WIDTH))
                || (lane != 0 && lane != 4)
            {
                assert!(validate(s, &a, &b, &v).is_err());
            }
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
    #[test]
    fn arithmetic_guards_stamps_and_echoes() {
        for mode in 0..2 {
            for bits in [16, 32] {
                let mut b = [POISON; MATH_BYTES];
                for i in 0..ROWS {
                    let p = 64 + i * MATH_STRIDE;
                    if !row_enabled(mode << 8, i) {
                        continue;
                    }
                    for lane in 0..24 {
                        put(&mut b, p + lane * 4, 0);
                    }
                    put(&mut b, p, 0x554e_4900 ^ math_tag(mode, bits));
                    put(&mut b, p + 4, i as u32);
                    put(&mut b, p + 8, bits);
                    put(&mut b, p + 16, u32::from(DEST_R[i]));
                    put(&mut b, p + 60, SOURCE_R[i]);
                    put(&mut b, p + 88, 0x6861_6c66);
                    put(&mut b, p + 92, u32::from(DEST_R[i]));
                }
                assert_eq!(math_capture(&b, mode, bits), Ok(()));
                for n in 0..b.len() {
                    assert_eq!(math_capture(&b[..n], mode, bits), Err(Error::Length));
                }
                for offset in [0, 63, 64 + ROWS * MATH_STRIDE, MATH_BYTES - 1] {
                    let mut x = b;
                    x[offset] ^= 1;
                    assert_eq!(math_capture(&x, mode, bits), Err(Error::Guard));
                }
                assert_eq!(math_capture(&b, mode ^ 1, bits), Err(Error::Trace));
                let mut x = b;
                x[64 + 60] ^= 1;
                assert_eq!(math_capture(&x, mode, bits), Err(Error::Trace));
            }
        }
    }
    #[test]
    fn frontier_outputs_are_observations_not_an_exact_only_gate() {
        let s = SPECS[9];
        let (mut a, b, mut c) = fixture(s);
        for (row, h) in [
            (18, 0x0000u16),
            (20, 0x0001),
            (33, 0x7c00),
            (35, 0xfc00),
            (36, 0x7e23),
            (38, 0x8000),
        ] {
            let p = offset(1, row + 1);
            a[p..p + 2].copy_from_slice(&h.to_le_bytes());
            let p = 64 + ((row + 1) * WIDTH + 1) * 48;
            put(&mut c, p, expand(h));
            put(&mut c, p + 16, u32::from(h));
        }
        assert_eq!(validate(s, &a, &b, &c), Ok(()));
        let fast = mode_spec(s, 1);
        assert_eq!(validate(fast, &a, &b, &c), Err(Error::Trace));
    }
    #[test]
    fn finite_factor_rounds_and_exact_half_fma_is_general() {
        assert_eq!(half_model(65505.0f32.to_bits(), true), Some(0x7bff));
        assert_eq!(half_model(65505.0f32.to_bits(), false), Some(0x7bff));
        assert_eq!(half_fma(0xfbff, 0, 0x7bff, true), Some(0xfbff));
        assert_eq!(half_fma(0x4000, 0x4000, 0xbc00, true), Some(0));
        assert_eq!(half_fma(0x8000, 0x8000, 0x3c00, true), Some(0x8000));
        assert_eq!(half_fma(0x7c00, 0, 0xfc00, true), None);
    }
    #[test]
    fn fast_scope_is_explicit_and_normals_are_not_skipped() {
        assert_eq!((0..ROWS).filter(|&row| !row_enabled(256, row)).count(), 5);
        for base in SPECS.into_iter().filter(|s| capture_enabled(*s, 1)) {
            let s = mode_spec(base, 1);
            let (a, b, c) = fixture(s);
            assert_eq!(validate(s, &a, &b, &c), Ok(()), "{}", s.name);
        }
        let s = mode_spec(SPECS[9], 1);
        let (a, mut b, c) = fixture(s);
        b[64 + 32 * 32] = 0;
        assert_eq!(validate(s, &a, &b, &c), Err(Error::Guard));
        for row in [
            12, 13, 34, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60,
            61, 62, 63,
        ] {
            assert!(row_enabled(256, row));
        }
    }
}
