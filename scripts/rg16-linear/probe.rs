// Dependency-free host generator/acceptance/observations. Unknown models never PASS.
#![forbid(unsafe_code)]
mod air;
use std::{collections::BTreeMap, fs, path::Path};
type Result<T> = std::result::Result<T, String>;
const W: usize = 5;
const H: usize = 3;
const N: usize = 128;
const CASES: usize = 8;
const IMAGE: usize = 1280;
const TRACE: usize = 10688;
const READ: usize = 2560;
fn need(b: bool, s: &str) -> Result<()> {
    if b {
        Ok(())
    } else {
        Err(s.into())
    }
}
fn load(p: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
    need(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= 2_097_152,
        "bounded ordinary input file",
    )?;
    fs::read(p).map_err(|e| e.to_string())
}
fn save(p: &Path, b: impl AsRef<[u8]>) -> Result<()> {
    need(b.as_ref().len() <= 2_097_152, "output budget")?;
    fs::write(p, b).map_err(|e| e.to_string())
}
fn text(p: &Path) -> Result<String> {
    String::from_utf8(load(p)?).map_err(|e| e.to_string())
}
fn word(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn put(b: &mut [u8], p: usize, v: u32) {
    b[p..p + 4].copy_from_slice(&v.to_le_bytes());
}
fn finite(b: u32) -> bool {
    b & 0x7f80_0000 != 0x7f80_0000
}
fn expand(h: u16) -> u32 {
    let sign = u32::from(h & 0x8000) << 16;
    let e = (h >> 10) & 31;
    let f = u32::from(h & 1023);
    if e == 0 {
        if f == 0 {
            sign
        } else {
            let top = 31 - f.leading_zeros();
            sign | ((top + 103) << 23) | ((f - (1 << top)) << (23 - top))
        }
    } else if e == 31 {
        sign | 0x7f80_0000 | (f << 13)
    } else {
        sign | ((u32::from(e) + 112) << 23) | (f << 13)
    }
}
fn hf(h: u16) -> f32 {
    f32::from_bits(expand(h))
}
// Round signed integer * 2^exponent directly, with no f64/double rounding.
// mode 0=nearest-even, 1=nearest-away, 2=toward-zero. Owned finite inputs only.
fn encode(n: i128, exponent: i32, m: u32, eb: u32, mode: u32) -> u32 {
    let sign = if n < 0 { 1u32 << (m + eb) } else { 0 };
    let a = n.unsigned_abs();
    if a == 0 {
        return sign;
    }
    let top = 127 - a.leading_zeros();
    let bias = (1i32 << (eb - 1)) - 1;
    let e = top as i32 + exponent;
    let shift = (if e < 1 - bias {
        1 - bias - m as i32
    } else {
        e - m as i32
    }) - exponent;
    let mut mant = if shift <= 0 {
        a << (-shift as u32)
    } else if shift >= 128 {
        0
    } else {
        a >> (shift as u32)
    };
    if shift > 0 && shift < 128 && mode != 2 {
        let mask = (1u128 << shift) - 1;
        let rest = a & mask;
        let half = 1u128 << (shift - 1);
        if rest > half || (rest == half && (mode == 1 || mant & 1 != 0)) {
            mant += 1;
        }
    }
    let mut normal = e.max(1 - bias);
    if mant >= 2u128 << m {
        mant >>= 1;
        normal += 1;
    }
    let body = if normal > bias {
        if mode == 2 {
            (((1 << eb) - 1) << m) - 1
        } else {
            ((1 << eb) - 1) << m
        }
    } else if mant < 1u128 << m {
        mant as u32
    } else {
        ((normal + bias) as u32) << m | (mant as u32 - (1 << m))
    };
    sign | body
}
fn half(v: f32, mode: u32, positive_zero: bool) -> u16 {
    let b = v.to_bits();
    let e = ((b >> 23) & 255) as i32;
    let f = b & 0x7f_ffff;
    if e == 255 {
        return ((b >> 16) & 0x8000) as u16 | 0x7c00 | if f != 0 { 0x200 } else { 0 };
    }
    let n = if e == 0 { f } else { f | 0x80_0000 };
    let n = if b >> 31 != 0 {
        -(n as i128)
    } else {
        n as i128
    };
    let mut h = encode(n, if e == 0 { -149 } else { e - 150 }, 10, 5, mode) as u16;
    if h & 0x7fff == 0 {
        h = if positive_zero {
            0
        } else {
            ((b >> 16) & 0x8000) as u16
        };
    }
    h
}
fn units(h: u16) -> i128 {
    let e = (h >> 10) & 31;
    let m = if e == 0 { h & 1023 } else { 1024 + (h & 1023) };
    let n = i128::from(m) << e.saturating_sub(1);
    if h & 0x8000 != 0 {
        -n
    } else {
        n
    }
}
fn next(r: &mut u32) -> u32 {
    *r = r.wrapping_mul(1664525).wrapping_add(1013904223);
    *r
}
fn sum_ltr(t: [u16; 4], wx: u32, wy: u32) -> f32 {
    let v = t.map(hf);
    let w = [
        (256 - wx) * (256 - wy),
        wx * (256 - wy),
        (256 - wx) * wy,
        wx * wy,
    ]
    .map(|x| x as f32);
    ((v[0] * w[0] + v[1] * w[1]) + v[2] * w[2] + v[3] * w[3]) * (1.0 / 65536.0)
}
fn sum_pair(t: [u16; 4], wx: u32, wy: u32) -> f32 {
    let v = t.map(hf);
    let w = [
        (256 - wx) * (256 - wy),
        wx * (256 - wy),
        (256 - wx) * wy,
        wx * wy,
    ]
    .map(|x| x as f32);
    ((v[0] * w[0] + v[1] * w[1]) + (v[2] * w[2] + v[3] * w[3])) * (1.0 / 65536.0)
}
fn exact(t: [u16; 4], wx: u32, wy: u32) -> i128 {
    let w = [
        (256 - wx) * (256 - wy),
        wx * (256 - wy),
        (256 - wx) * wy,
        wx * wy,
    ];
    t.into_iter()
        .zip(w)
        .map(|(h, w)| units(h) * i128::from(w))
        .sum()
}
fn search(which: usize) -> ([u16; 4], usize) {
    let mut seed = 0x7302_14abu32 ^ which as u32;
    let (wx, wy) = if which == 0 { (173, 103) } else { (87, 219) };
    for i in 0..1_000_000 {
        let taps = std::array::from_fn(|_| {
            let r = next(&mut seed);
            ((r >> 16) as u16 & 0x8000) | ((((r >> 10) % 24 + 2) as u16) << 10) | (r as u16 & 1023)
        });
        let a = half(sum_ltr(taps, wx, wy), 1, true);
        let b = if which == 0 {
            half(sum_pair(taps, wx, wy), 1, true)
        } else {
            encode(exact(taps, wx, wy), -40, 10, 5, 1) as u16
        };
        if a != b && a & 0x7c00 != 0x7c00 && b & 0x7c00 != 0x7c00 {
            return (taps, i + 1);
        }
    }
    panic!("bounded owned discriminator search exhausted")
}
fn cases() -> Vec<[[u16; 2]; W * H]> {
    let mut out = vec![[[0; 2]; W * H]; CASES];
    let at = [1, 2, W + 1, W + 2];
    out[0][at[0]][0] = 0x3c00;
    out[0][at[1]][1] = 0x3c00;
    out[1][at[2]][0] = 0x3c00;
    out[1][at[3]][1] = 0x3c00;
    for (i, h) in [0x3c00, 0x3555, 0xba00, 0x2c00].into_iter().enumerate() {
        out[2][at[i]][0] = h;
        out[3][at[i]][1] = h;
    }
    for (i, p) in out[4].iter_mut().enumerate() {
        let h = 0x3c00 + u16::from(i % W == 2);
        *p = [h, h | 0x8000];
    }
    let edge = [
        0, 0x8000, 1, 0x8001, 0x03ff, 0x83ff, 0x0400, 0x8400, 0x7bff, 0xfbff, 0x3555, 0xb555,
        0x3c01, 0xbc01, 0x3c00,
    ];
    for i in 0..W * H {
        out[5][i] = [edge[i], edge[(i + 3) % (W * H)]];
    }
    for case in 6..8 {
        let (t, attempts) = search(case - 6);
        let pinned = [
            ([0x9692, 0xc4c9, 0x2794, 0x8de3], 52814),
            ([0x3775, 0xe250, 0x316f, 0x1002], 14350),
        ];
        assert_eq!(
            (t, attempts),
            pinned[case - 6],
            "owned discriminator identity changed"
        );
        for i in 0..4 {
            out[case][at[i]] = [t[i], t[3 - i] ^ 0x8000];
        }
    }
    out
}
#[derive(Clone, Copy)]
struct Coord {
    pixel: [f32; 2],
    norm: [f32; 2],
    class: u32,
}
fn coords() -> Vec<Coord> {
    let mut v = Vec::new();
    let mut add = |p: [f32; 2], class: u32| {
        v.push(Coord {
            pixel: p,
            norm: [p[0] / W as f32, p[1] / H as f32],
            class,
        });
    };
    add([1.5, 0.5], 0);
    for y in 0..H {
        for x in 0..W {
            add([x as f32 + 0.5, y as f32 + 0.5], 1);
        }
    }
    for p in [
        [-1., -1.],
        [0., 0.],
        [-0., -0.],
        [5., 3.],
        [6., 4.],
        [-1., 1.25],
        [6., 1.25],
        [2.25, -1.],
        [2.25, 4.],
        [0.25, 0.25],
        [4.75, 2.75],
        [0.5, 2.5],
    ] {
        add(p, 2);
    }
    drop(add);
    for axis in 0..2 {
        for k in [0, 1, 63, 127, 128, 191, 254, 255] {
            for delta in [-1i32, 0, 1] {
                let mut p = [1.5, 0.5];
                p[axis] += (k as f32 + 0.5) / 256.;
                let mut n = [p[0] / W as f32, p[1] / H as f32];
                p[axis] = f32::from_bits((i64::from(p[axis].to_bits()) + i64::from(delta)) as u32);
                n[axis] = f32::from_bits((i64::from(n[axis].to_bits()) + i64::from(delta)) as u32);
                v.push(Coord {
                    pixel: p,
                    norm: n,
                    class: 3 + axis as u32,
                });
            }
        }
    }
    for fy in [1., 33., 64., 127.5, 128., 192., 255.] {
        for fx in [1., 33., 64., 127.5, 128., 192., 255.] {
            let p = [1.5 + fx / 256., 0.5 + fy / 256.];
            v.push(Coord {
                pixel: p,
                norm: [p[0] / W as f32, p[1] / H as f32],
                class: 5,
            });
        }
    }
    for (wx, wy) in [(173., 103.), (87., 219.), (127., 129.)] {
        let p = [1.5 + wx / 256., 0.5 + wy / 256.];
        v.push(Coord {
            pixel: p,
            norm: [p[0] / W as f32, p[1] / H as f32],
            class: 6,
        });
    }
    assert_eq!(v.len(), N);
    v
}
fn coordinate_bytes(c: &[Coord], norm: usize) -> Vec<u8> {
    let mut out = vec![0; N * 16];
    for (i, c) in c.iter().enumerate() {
        let p = if norm == 0 { c.pixel } else { c.norm };
        for (j, v) in [p[0].to_bits(), p[1].to_bits(), i as u32, c.class]
            .into_iter()
            .enumerate()
        {
            put(&mut out, i * 16 + j * 4, v);
        }
    }
    out
}
fn image_bytes(c: &[[u16; 2]; W * H], channels: usize) -> Vec<u8> {
    let mut b = vec![0xcd; IMAGE];
    for (y, row) in c.chunks(W).enumerate() {
        for (x, p) in row.iter().enumerate() {
            for (k, h) in [p[0], p[1], 0, 0x3c00]
                .into_iter()
                .take(channels)
                .enumerate()
            {
                let at = 256 + y * 256 + (x * channels + k) * 2;
                b[at..at + 2].copy_from_slice(&h.to_le_bytes());
            }
        }
    }
    b
}
fn name(case: usize, norm: usize, bits: u32) -> String {
    format!("c{case}-s{norm}-f{bits}.bin")
}
fn tag(case: usize, norm: usize, bits: u32, mode: u32) -> u32 {
    (mode << 12) | ((case as u32) << 4) | ((norm as u32) << 2) | if bits == 16 { 1 } else { 2 }
}
fn generate(root: &Path) -> Result<()> {
    let cs = cases();
    let uv = coords();
    for norm in 0..2 {
        save(
            &root.join(format!("coordinates-{norm}.bin")),
            coordinate_bytes(&uv, norm),
        )?;
    }
    for (id, c) in cs.iter().enumerate() {
        for (ch, suffix) in [(2, "rg"), (4, "control"), (1, "red")] {
            save(
                &root.join(format!("c{id}-{suffix}-upload.bin")),
                image_bytes(c, ch),
            )?;
        }
    }
    save(&root.join("plan.json"),format!("{{\"schema\":1,\"width\":{W},\"height\":{H},\"rows\":{N},\"cases\":{CASES},\"pitch\":256,\"imageBytes\":{IMAGE},\"traceBytes\":{TRACE},\"readBytes\":{READ},\"offset\":[0,0],\"finite_only\":true,\"formats\":[65,115,25],\"format_roles\":[\"rg\",\"control\",\"red\"],\"samplers\":[\"pixel-edge-linear\",\"normalized-edge-linear\"]}}\n"))?;
    let mut t=String::from("case\trole\n0\ttaps00-R_10-G\n1\ttaps01-R_11-G\n2\tR-only\n3\tG-only\n4\tpositive-negative-half-ties\n5\tfinite-signed-zero-subnormal-extreme\n6\towned-LTR-versus-pair\n7\towned-LTR-versus-exact\n");
    for i in 0..2 {
        let (taps, n) = search(i);
        t += &format!("# search{} attempts={} taps={:04x?}\n", i, n, taps);
    }
    save(&root.join("corpus.tsv"), t)?;
    let mut pre=String::from("model\tcomponents\tdisagreements_vs_reference_candidate\tfirst_case_sampler_bits_row_channel\n");
    for m in 0..MODELS {
        let mut different = 0;
        let mut total = 0;
        let mut first = String::from("none");
        for (id, c) in cs.iter().enumerate() {
            for norm in 0..2 {
                for bits in [16, 32] {
                    for (row, co) in uv.iter().enumerate() {
                        for ch in 0..2 {
                            total += 1;
                            if predict(c, *co, norm, ch, bits, m)
                                != predict(c, *co, norm, ch, bits, 2)
                            {
                                different += 1;
                                if first == "none" {
                                    first = format!("{id},{norm},{bits},{row},{ch}");
                                }
                            }
                        }
                    }
                }
            }
        }
        pre += &format!("{}\t{total}\t{different}\t{first}\n", model_name(m));
    }
    save(&root.join("candidate-preflight.tsv"), pre)?;
    Ok(())
}
fn check_inputs(root: &Path) -> Result<(Vec<[[u16; 2]; W * H]>, Vec<Coord>)> {
    let cs = cases();
    let uv = coords();
    for norm in 0..2 {
        need(
            load(&root.join(format!("coordinates-{norm}.bin")))? == coordinate_bytes(&uv, norm),
            "owned coordinate identity",
        )?;
    }
    for (id, c) in cs.iter().enumerate() {
        for (ch, suffix) in [(2, "rg"), (4, "control"), (1, "red")] {
            need(
                load(&root.join(format!("c{id}-{suffix}-upload.bin")))? == image_bytes(c, ch),
                "owned texel identity",
            )?;
        }
    }
    Ok((cs, uv))
}
fn check_trace(
    b: &[u8],
    input: &[u8],
    case: usize,
    norm: usize,
    bits: u32,
    mode: u32,
    negative: bool,
) -> Result<()> {
    need(b.len() == TRACE && input.len() == N * 16, "trace size")?;
    let t = tag(case, norm, bits, mode);
    need(
        b[..64]
            .iter()
            .chain(b[64 + N * 80..].iter())
            .all(|x| *x == 0xcd),
        "trace prefix/suffix/guard rows",
    )?;
    for i in 0..N {
        let p = 64 + i * 80;
        if negative && i == 0 {
            need(
                b[p..p + 80].iter().all(|x| *x == 0xcd),
                "native single skip",
            )?;
            continue;
        }
        need(
            [word(b, p), word(b, p + 4), word(b, p + 8), word(b, p + 12)]
                == [0x5247_4600 ^ t, i as u32, bits, case as u32],
            "trace stamp",
        )?;
        need(
            b[p + 16..p + 32] == input[i * 16..i * 16 + 16],
            "coordinate bits/tag echo",
        )?;
        for j in 0..12 {
            let w = word(b, p + 32 + j * 4);
            need(
                if bits == 16 {
                    w <= 65535 && w & 0x7c00 != 0x7c00
                } else {
                    finite(w)
                },
                "finite sampler result width",
            )?;
        }
        for k in [2, 10] {
            need(
                word(b, p + 32 + k * 4) == 0
                    && word(b, p + 36 + k * 4) == if bits == 16 { 0x3c00 } else { 0x3f80_0000 },
                "canonical missing B/A (stored control B/A are observations)",
            )?;
        }
        need(word(b, p + 68) == 0, "canonical missing R16 G")?;
        // Pixel center is exact. Do not assume normalized 0.3 is an exact center.
        if case == 1 && norm == 1 && i == 8 {
            for offset in [32, 48] {
                need(
                    word(b, p + offset) == 0
                        && word(b, p + offset + 4) == if bits == 16 { 0x3c00 } else { 0x3f80_0000 },
                    "exact normalized (0.5,0.5) basis anchor",
                )?;
            }
            need(word(b, p + 64) == 0, "exact normalized R16 zero anchor")?;
        }
        // Only known exact ordinary finite anchor, not a rounded-model oracle.
        if case == 0 && norm == 0 && i == 0 {
            for offset in [32, 48, 64] {
                need(
                    word(b, p + offset) == if bits == 16 { 0x3c00 } else { 0x3f80_0000 }
                        && word(b, p + offset + 4) == 0,
                    "exact basis anchor",
                )?;
            }
        }
    }
    Ok(())
}
fn check_reads(b: &[u8], case: usize, mode: u32, c: &[[u16; 2]; W * H]) -> Result<()> {
    need(b.len() == READ, "read size")?;
    let mut expected = vec![0xcd; READ];
    let tag = 0x5000 | (mode << 8) | case as u32;
    for (y, row) in c.chunks(W).enumerate() {
        for (x, p) in row.iter().enumerate() {
            let i = y * W + x;
            let at = 64 + i * 128;
            let h = [p[0], p[1], 0, 0x3c00];
            let f = h.map(expand);
            let w = h.map(u32::from);
            let all = [
                vec![0x5247_5200 ^ tag, i as u32, x as u32, y as u32],
                f.to_vec(),
                w.to_vec(),
                f.to_vec(),
                w.to_vec(),
                vec![expand(p[0]), 0, 0, 0x3f80_0000],
                vec![u32::from(p[0]), 0, 0, 0x3c00],
                vec![W as u32, H as u32, case as u32, 0x7265_6164],
            ]
            .concat();
            for (j, v) in all.into_iter().enumerate() {
                put(&mut expected, at + j * 4, v);
            }
        }
    }
    need(b == expected, "raw typed read/guard/stamp identity")
}
// Deliberately competing hypotheses, NOT acceptance oracles. Reference candidate:
// q8 half-up, four-term f32 LTR, final half nearest-away/+0, both return widths.
const MODELS: usize = 30;
fn model_name(m: usize) -> String {
    if m < 24 {
        format!(
            "q8-up/{}:{}",
            [
                "ltr32",
                "pair32",
                "row-lerp32",
                "col-lerp32",
                "row-fma32",
                "exact"
            ][m / 4],
            ["f32-or-half-rne", "half-rne+0", "half-rna+0", "half-rtz+0"][m % 4]
        )
    } else {
        format!(
            "{}/ltr32:half-rna+0",
            [
                "q8-even",
                "q8-floor",
                "q10-up",
                "unquantized",
                "promoted-coordinate-q8-up",
                "promoted-coordinate-q8-even"
            ][m - 24]
        )
    }
}
fn predict(
    c: &[[u16; 2]; W * H],
    co: Coord,
    norm: usize,
    channel: usize,
    bits: u32,
    m: usize,
) -> u32 {
    let uv = if norm == 0 { co.pixel } else { co.norm };
    let sizes = [W as f32, H as f32];
    let mut pos = [0.; 2];
    for i in 0..2 {
        pos[i] = if norm == 0 {
            uv[i].clamp(-1., sizes[i] + 1.)
        } else {
            uv[i].clamp(0., 1.) * sizes[i]
        };
        pos[i] -= 0.5;
    }
    let mut base = pos.map(|v| v.floor() as i32);
    let mut f = [pos[0] - base[0] as f32, pos[1] - base[1] as f32];
    if m >= 28 {
        for i in 0..2 {
            let u = f64::from(uv[i]);
            let size = f64::from(sizes[i]);
            let p = if norm == 0 {
                u.clamp(-1., size + 1.)
            } else {
                u.clamp(0., 1.) * size
            } - 0.5;
            base[i] = p.floor() as i32;
            let q = (p - f64::from(base[i])) * 256.;
            f[i] = (if m == 28 {
                q.floor() + if q - q.floor() >= 0.5 { 1.0 } else { 0.0 }
            } else {
                q.round_ties_even()
            }) as f32
                / 256.;
        }
    } else {
        let quant = if m < 24 { 0 } else { m - 23 };
        for v in &mut f {
            if quant != 4 {
                let scale = if quant == 3 { 1024. } else { 256. };
                let q = *v * scale;
                *v = if quant == 1 {
                    q.round_ties_even()
                } else if quant == 2 {
                    q.floor()
                } else {
                    q.floor() + if q - q.floor() >= 0.5 { 1.0 } else { 0.0 }
                };
                *v /= scale;
            }
        }
    }
    let t = std::array::from_fn::<_, 4, _>(|i| {
        let x = (base[0] + (i % 2) as i32).clamp(0, W as i32 - 1) as usize;
        let y = (base[1] + (i / 2) as i32).clamp(0, H as i32 - 1) as usize;
        c[y * W + x][channel]
    });
    let v = t.map(hf);
    let [x, y] = f;
    let ar = if m < 24 { m / 4 } else { 0 };
    let boundary = if m < 24 { m % 4 } else { 2 };
    let mut raw = if x == 0. && y == 0. {
        v[0]
    } else {
        let w = [(1. - x) * (1. - y), x * (1. - y), (1. - x) * y, x * y];
        match ar {
            0 => ((v[0] * w[0] + v[1] * w[1]) + v[2] * w[2]) + v[3] * w[3],
            1 => (v[0] * w[0] + v[1] * w[1]) + (v[2] * w[2] + v[3] * w[3]),
            2 => {
                let a = v[0] * (1. - x) + v[1] * x;
                let b = v[2] * (1. - x) + v[3] * x;
                a * (1. - y) + b * y
            }
            3 => {
                let a = v[0] * (1. - y) + v[2] * y;
                let b = v[1] * (1. - y) + v[3] * y;
                a * (1. - x) + b * x
            }
            4 => {
                let a = (v[1] - v[0]).mul_add(x, v[0]);
                let b = (v[3] - v[2]).mul_add(x, v[2]);
                (b - a).mul_add(y, a)
            }
            _ => f32::from_bits(encode(
                exact(t, (x * 256.) as u32, (y * 256.) as u32),
                -40,
                23,
                8,
                0,
            )),
        }
    };
    if ar == 5 && boundary != 0 {
        let h = encode(
            exact(t, (x * 256.) as u32, (y * 256.) as u32),
            -40,
            10,
            5,
            (boundary - 1) as u32,
        ) as u16;
        raw = hf(if h & 0x7fff == 0 { 0 } else { h });
    } else if boundary != 0 {
        raw = hf(half(raw, (boundary - 1) as u32, true));
    }
    if bits == 16 {
        u32::from(half(raw, 0, false))
    } else {
        raw.to_bits()
    }
}
fn table(root: &Path, n: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let s = text(&root.join(n))?;
    let mut lines = s.lines();
    let header: Vec<_> = lines
        .next()
        .ok_or("missing table header")?
        .split('\t')
        .collect();
    lines
        .map(|l| {
            let a: Vec<_> = l.split('\t').collect();
            need(a.len() == header.len(), "table columns")?;
            Ok(header
                .iter()
                .zip(a)
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect())
        })
        .collect()
}
fn val<'a>(r: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str> {
    r.get(k)
        .map(String::as_str)
        .ok_or_else(|| format!("missing column {k}"))
}
fn mode_index(root: &Path) -> Result<u32> {
    match root.file_name().and_then(|n| n.to_str()) {
        Some("precise") => Ok(0),
        Some("fast") => Ok(1),
        _ => Err("result directory must name precise or fast; no implicit mode".into()),
    }
}
fn validate(root: &Path) -> Result<()> {
    let (cs, uv) = check_inputs(root)?;
    let mode = mode_index(root)?;
    let library = air::sha(&load(&root.join("program.metallib"))?);
    let mut input = load(&root.join("plan.json"))?;
    for norm in 0..2 {
        input.extend(coordinate_bytes(&uv, norm));
    }
    for c in &cs {
        input.extend(image_bytes(c, 2));
        input.extend(image_bytes(c, 4));
        input.extend(image_bytes(c, 1));
    }
    let input_hash = air::sha(&input);
    let mut count = 0;
    let receipts = table(root, "gpu.tsv")?;
    need(receipts.len() == 41, "native CB count")?;
    let mut expected = BTreeMap::new();
    for case in 0..CASES {
        expected.insert(
            format!("c{case}-read.bin"),
            (
                "typed-read",
                case,
                -1,
                0,
                0x5000 | (mode << 8) | case as u32,
            ),
        );
        for norm in 0..2 {
            for bits in [16, 32] {
                expected.insert(
                    name(case, norm, bits),
                    (
                        "sample",
                        case,
                        norm as i32,
                        bits,
                        tag(case, norm, bits, mode),
                    ),
                );
            }
        }
    }
    expected.insert(
        "negative-skip.bin".into(),
        ("negative-skip", 0, 0, 32, tag(0, 0, 32, mode)),
    );
    for r in &receipts {
        need(
            val(r, "status")? == "4"
                && val(r, "error")? == "0"
                && val(r, "inputs_unchanged")? == "1"
                && val(r, "library_sha256")? == library
                && val(r, "inputs_sha256")? == input_hash,
            "native receipt status/library/inputs",
        )?;
        let name = val(r, "file")?;
        let (kind, case, norm, bits, tag) = expected
            .remove(name)
            .ok_or("duplicate or unlisted receipt")?;
        need(
            val(r, "kind")? == kind
                && val(r, "case")? == case.to_string()
                && val(r, "sampler")? == norm.to_string()
                && val(r, "return_bits")? == bits.to_string()
                && val(r, "tag")? == tag.to_string(),
            "receipt dispatch identity",
        )?;
        need(
            air::sha(&load(&root.join(name))?) == val(r, "sha256")?,
            "raw receipt identity",
        )?;
        let start: f64 = val(r, "gpu_start")?.parse().map_err(|_| "GPU time")?;
        let end: f64 = val(r, "gpu_end")?.parse().map_err(|_| "GPU time")?;
        need(
            start.is_finite() && end.is_finite() && start > 0. && end >= start,
            "GPU completion times",
        )?;
    }
    need(expected.is_empty(), "receipt coverage")?;
    let mut observations=String::from("case\tsampler\treturn_bits\trow\tu_bits\tv_bits\tclass\trg_R\trg_G\trg_B\trg_A\tcontrol_R\tcontrol_G\tcontrol_B\tcontrol_A\tred_R\tred_G\tred_B\tred_A\n");
    let mut matches=String::from("case\tsampler\treturn_bits\tmodel\tcomponents_RG_control\tcomponents_R16\trg_mismatches\tcontrol_mismatches\tred_mismatches\n");
    let mut cross=String::from("case\tsampler\treturn_bits\trows\tRG_vs_control_components_different\tR16_vs_control_red_different\tcontrol_BA_noncanonical_components\n");
    for (case, c) in cs.iter().enumerate() {
        check_reads(
            &load(&root.join(format!("c{case}-read.bin")))?,
            case,
            mode,
            c,
        )?;
        for (ch, suffix) in [(2, "rg"), (4, "control"), (1, "red")] {
            need(
                load(&root.join(format!("c{case}-{suffix}-copy.bin")))? == image_bytes(c, ch),
                "Private copy/host padding guards",
            )?;
        }
        for norm in 0..2 {
            let input = coordinate_bytes(&uv, norm);
            for bits in [16, 32] {
                let b = load(&root.join(name(case, norm, bits)))?;
                check_trace(&b, &input, case, norm, bits, mode, false)?;
                let mut errors = [[0usize; 3]; MODELS];
                let mut differing = 0;
                let mut red_differing = 0;
                let mut control_ba = 0;
                for (i, co) in uv.iter().enumerate() {
                    let p = 64 + i * 80;
                    let actual = std::array::from_fn::<_, 12, _>(|j| word(&b, p + 32 + j * 4));
                    observations += &format!(
                        "{case}\t{norm}\t{bits}\t{i}\t{:08x}\t{:08x}\t{}",
                        word(&input, i * 16),
                        word(&input, i * 16 + 4),
                        co.class
                    );
                    for w in actual {
                        observations += &format!("\t{w:08x}");
                    }
                    observations.push('\n');
                    for channel in 0..2 {
                        if actual[channel] != actual[4 + channel] {
                            differing += 1;
                        }
                        for (m, e) in errors.iter_mut().enumerate() {
                            let want = predict(c, *co, norm, channel, bits, m);
                            for k in 0..3 {
                                if k < 2 || channel == 0 {
                                    e[k] += usize::from(actual[k * 4 + channel] != want);
                                }
                            }
                        }
                    }
                    if actual[8] != actual[4] {
                        red_differing += 1;
                    }
                    control_ba += usize::from(actual[6] != 0)
                        + usize::from(actual[7] != if bits == 16 { 0x3c00 } else { 0x3f80_0000 });
                    count += 1;
                }
                cross += &format!(
                    "{case}\t{norm}\t{bits}\t{N}\t{differing}\t{red_differing}\t{control_ba}\n"
                );
                for (m, e) in errors.iter().enumerate() {
                    matches += &format!(
                        "{case}\t{norm}\t{bits}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        model_name(m),
                        N * 2,
                        N,
                        e[0],
                        e[1],
                        e[2]
                    );
                }
            }
        }
    }
    let input = coordinate_bytes(&uv, 0);
    let neg = load(&root.join("negative-skip.bin"))?;
    check_trace(&neg, &input, 0, 0, 32, mode, true)?;
    need(
        check_trace(&neg, &input, 0, 0, 32, mode, false).is_err(),
        "native skip must fail ordinary gate",
    )?;
    let good = load(&root.join(name(0, 0, 32)))?;
    let mut negatives = String::from("control\tordinary_gate\nnative-skip\trejected\n");
    for (at, label) in [
        (0, "prefix"),
        (64, "stamp"),
        (80, "coordinate"),
        (96, "exact-anchor"),
        (104, "missing-B"),
        (64 + N * 80, "guard-row"),
    ] {
        let mut bad = good.clone();
        bad[at] ^= 1;
        need(
            check_trace(&bad, &input, 0, 0, 32, mode, false).is_err(),
            "corrupt negative not rejected",
        )?;
        negatives += &format!("{label}\trejected\n");
    }
    let mut precision=String::from("case\tsampler\tcomponents_three_formats\tfloat_not_exact_half\thalf_not_rne_of_float\thalf_not_bit_equal_to_float\n");
    for case in 0..CASES {
        for norm in 0..2 {
            let f = load(&root.join(name(case, norm, 32)))?;
            let h = load(&root.join(name(case, norm, 16)))?;
            let mut counts = [0; 3];
            for row in 0..N {
                for ch in [0, 1, 4, 5, 8] {
                    let at = 64 + 80 * row + 32 + ch * 4;
                    let fw = word(&f, at);
                    let hw = word(&h, at) as u16;
                    let rn = half(f32::from_bits(fw), 0, false);
                    counts[0] += usize::from(fw != expand(rn));
                    counts[1] += usize::from(hw != rn);
                    counts[2] += usize::from(fw != expand(hw));
                }
            }
            precision += &format!(
                "{case}\t{norm}\t{}\t{}\t{}\t{}\n",
                N * 5,
                counts[0],
                counts[1],
                counts[2]
            );
        }
    }
    save(&root.join("observations.tsv"), observations)?;
    save(&root.join("candidate-matches.tsv"), matches)?;
    save(&root.join("format-comparison.tsv"), cross)?;
    save(&root.join("return-precision.tsv"), precision)?;
    save(&root.join("negatives.tsv"), negatives)?;
    save(&root.join("acceptance.txt"),format!("CAPTURE_INTEGRITY_PASS_PROFILE_UNSELECTED\npositive_sample_records={count}\nsample_values_three_formats={}\nnative_negative_records={}\ntyped_texel_records={}\nnegative_controls=7\nmodels={MODELS}\nmath_profile={}\nno_unmeasured_Preserve_claim=true\n",count*3,N-1,CASES*W*H,if mode==0{"FlushBinary32"}else{"FastFlushBinary32"}))?;
    Ok(())
}
fn extract(root: &Path) -> Result<()> {
    let b = load(&root.join("program.metallib"))?;
    let modules = air::extract(&b)?;
    need(modules.len() == air::ENTRIES.len(), "exact entry count")?;
    for (n, k) in air::ENTRIES {
        let m = modules
            .iter()
            .find(|m| m.name == n && m.stage == k)
            .ok_or("entry stage absent")?;
        save(&root.join(format!("{n}.air")), m.bytes)?;
    }
    Ok(())
}
fn bind(root: &Path) -> Result<()> {
    let b = load(&root.join("program.metallib"))?;
    let modules = air::extract(&b)?;
    need(modules.len() == 4, "entry count")?;
    let fast = mode_index(root)? == 1;
    let mut out =
        String::from("entry\tstage\toffset\tbytes\tair_sha256\tll_sha256\tlibrary_sha256\n");
    for (n, k) in air::ENTRIES {
        let m = modules
            .iter()
            .find(|m| m.name == n && m.stage == k)
            .ok_or("entry absent")?;
        need(
            load(&root.join(format!("{n}.air")))? == m.bytes,
            "extracted byte identity",
        )?;
        let ll = text(&root.join(format!("{n}.ll")))?;
        save(
            &root.join(format!("{n}-proof.txt")),
            air::proof(&ll, n, k, fast)?,
        )?;
        out += &format!(
            "{n}\t{k}\t{}\t{}\t{}\t{}\t{}\n",
            m.offset,
            m.bytes.len(),
            air::sha(m.bytes),
            air::sha(ll.as_bytes()),
            air::sha(&b)
        );
    }
    save(&root.join("bindings.tsv"), out)
}
fn compare(root: &Path) -> Result<()> {
    let a = text(&root.join("precise/observations.tsv"))?;
    let b = text(&root.join("fast/observations.tsv"))?;
    let a: Vec<_> = a.lines().collect();
    let b: Vec<_> = b.lines().collect();
    need(
        a.len() == 4097 && b.len() == a.len() && a[0] == b[0],
        "mode observation counts",
    )?;
    let different = a.iter().zip(&b).skip(1).filter(|(a, b)| a != b).count();
    let mut out=format!("positive_records=4096\nrows_different={different}\ncomparison_is_observation_not_oracle=true\n");
    for (i, (a, b)) in a
        .iter()
        .zip(&b)
        .enumerate()
        .skip(1)
        .filter(|(_, (a, b))| a != b)
        .take(64)
    {
        out += &format!("row={i}\nprecise={a}\nfast={b}\n");
    }
    save(&root.join("mode-comparison.txt"), out)
}
fn run() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    need(
        a.len() == 3,
        "generate|extract|bind|validate|compare directory",
    )?;
    let root = Path::new(&a[2]);
    need(
        root.is_dir()
            && !fs::symlink_metadata(root)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink(),
        "ordinary result directory",
    )?;
    match a[1].as_str() {
        "generate" => generate(root),
        "extract" => extract(root),
        "bind" => bind(root),
        "validate" => validate(root),
        "compare" => compare(root),
        _ => Err("unknown command".into()),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("UNQUALIFIED: {e}");
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_rounding_and_all_finite_words() {
        assert_eq!(expand(1), 0x3380_0000);
        assert_eq!(expand(0x0400), 0x3880_0000);
        assert_eq!(expand(0x7bff), 0x477f_e000);
        for h in 0..=u16::MAX {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(half(hf(h), 0, false), h);
            }
        }
        assert_eq!(half(1. + 1. / 2048., 0, true), 0x3c00);
        assert_eq!(half(1. + 1. / 2048., 1, true), 0x3c01);
        assert_eq!(half(-1. - 1. / 2048., 1, true), 0xbc01);
        assert_eq!(half(-0., 1, true), 0);
        assert_eq!(half(65520., 2, true), 0x7bff);
    }
    #[test]
    fn bounded_corpus_discriminates_after_output_rounding() {
        let c = cases();
        assert_eq!(c.len(), CASES);
        let uv = coords();
        assert_eq!(uv.len(), N);
        for id in 6..8 {
            let co = uv[125 + id - 6];
            let a = predict(&c[id], co, 0, 0, 32, 2);
            let alt = if id == 6 { 6 } else { 22 };
            assert_ne!(a, predict(&c[id], co, 0, 0, 32, alt));
        }
        for c in &c {
            assert!(c.iter().flatten().all(|h| h & 0x7c00 != 0x7c00));
        }
        assert_ne!(predict(&c[4], uv[100], 0, 0, 32, 1), 0xdeadbeef);
    }
    #[test]
    fn weight_coordinate_and_output_discriminators() {
        let c = cases();
        let uv = coords();
        // Exact q8 midpoint: nearest-away and nearest-even weights disagree.
        assert_eq!(predict(&c[0], uv[29], 0, 1, 32, 2), 0x3b80_0000);
        assert_eq!(predict(&c[0], uv[29], 0, 1, 32, 24), 0);
        assert_eq!(predict(&c[0], uv[28], 0, 1, 32, 2), 0);
        // Asymmetric non-power-of-two normalized coordinates expose promotion.
        assert_ne!(
            predict(&c[0], uv[32], 1, 0, 16, 2),
            predict(&c[0], uv[32], 1, 0, 16, 28)
        );
        assert!(uv
            .iter()
            .any(|co| predict(&c[4], *co, 0, 0, 32, 1) != predict(&c[4], *co, 0, 0, 32, 2)));
    }
    fn fake() -> Vec<u8> {
        let input = coordinate_bytes(&coords(), 0);
        let mut b = vec![0xcd; TRACE];
        for i in 0..N {
            let p = 64 + i * 80;
            for (j, v) in [0x5247_4600 ^ tag(0, 0, 32, 0), i as u32, 32, 0]
                .into_iter()
                .enumerate()
            {
                put(&mut b, p + j * 4, v);
            }
            b[p + 16..p + 32].copy_from_slice(&input[i * 16..i * 16 + 16]);
            for j in 0..12 {
                put(
                    &mut b,
                    p + 32 + j * 4,
                    if j % 4 == 3 || i == 0 && j % 4 == 0 {
                        0x3f80_0000
                    } else {
                        0
                    },
                );
            }
        }
        b
    }
    #[test]
    fn strict_known_gates_and_unknown_rows() {
        let input = coordinate_bytes(&coords(), 0);
        let good = fake();
        assert!(check_trace(&good, &input, 0, 0, 32, 0, false).is_ok());
        for p in [0, 64, 80, 96, 104, TRACE - 1, 64 + N * 80] {
            let mut bad = good.clone();
            bad[p] ^= 1;
            assert!(check_trace(&bad, &input, 0, 0, 32, 0, false).is_err());
        }
        let mut unknown = good.clone();
        put(&mut unknown, 64 + 80 + 32, 1.2345f32.to_bits());
        assert!(check_trace(&unknown, &input, 0, 0, 32, 0, false).is_ok());
    }
    #[test]
    fn native_skip_control_uses_the_same_tag() {
        let input = coordinate_bytes(&coords(), 0);
        let mut b = fake();
        b[64..144].fill(0xcd);
        assert!(check_trace(&b, &input, 0, 0, 32, 0, true).is_ok());
        assert!(check_trace(&b, &input, 0, 0, 32, 0, false).is_err());
    }
    #[test]
    fn image_padding_and_all_taps() {
        let c = cases();
        for case in &c {
            for channels in [1, 2, 4] {
                let b = image_bytes(case, channels);
                assert_eq!(b.len(), IMAGE);
                assert!(b[..256].iter().chain(b[1024..].iter()).all(|v| *v == 0xcd));
                for y in 0..H {
                    assert!(b[256 + y * 256 + W * channels * 2..512 + y * 256]
                        .iter()
                        .all(|v| *v == 0xcd));
                }
            }
        }
    }
}
