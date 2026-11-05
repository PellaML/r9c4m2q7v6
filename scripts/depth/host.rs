//! Standalone, dependency-free generic position/depth experiment.
//! Portable reference data is never passed off as GPU output.
use std::fmt::{self, Write as _};
use std::fs;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::path::Path;

const WIDTH: usize = 8;
const MAX_VERTICES: usize = 12;
const GUARD: usize = 64;
const HEADER: usize = 16;
const VWORDS: usize = 16;
const PWORDS: usize = 16;
const PIXEL_BASE: usize = GUARD + HEADER + MAX_VERTICES * VWORDS;
const WORDS: usize = PIXEL_BASE + WIDTH * WIDTH * PWORDS + GUARD;
const SENTINEL: u32 = 0xa5a5_a5a5;
const VERTEX_SEEN: u32 = 0x7658_0001;
const SHADER: &str = include_str!("probe.metal");
type Result<T> = std::result::Result<T, String>;

// Exact, reduced integer rationals. The generated domain is deliberately small;
// this is not a general-purpose arbitrary-exponent IEEE-754 emulator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rat {
    n: i128,
    d: i128,
}
fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}
impl Rat {
    const ZERO: Self = Self { n: 0, d: 1 };
    const ONE: Self = Self { n: 1, d: 1 };
    fn new(mut n: i128, mut d: i128) -> Self {
        assert_ne!(d, 0);
        if d < 0 {
            n = -n;
            d = -d;
        }
        let g = gcd(n, d);
        Self { n: n / g, d: d / g }
    }
    fn int(n: i128) -> Self {
        Self::new(n, 1)
    }
    fn pow2(exponent: i32) -> Self {
        if exponent >= 0 {
            Self::int(1i128 << exponent)
        } else {
            Self::new(1, 1i128 << -exponent)
        }
    }
    #[cfg(test)]
    fn abs(self) -> Self {
        Self::new(self.n.abs(), self.d)
    }
    fn from_bits(bits: u32) -> Option<Self> {
        let exponent = ((bits >> 23) & 255) as i32;
        if exponent == 255 {
            return None;
        }
        let fraction = bits & 0x7f_ffff;
        if exponent == 0 && fraction == 0 {
            return Some(Self::ZERO);
        }
        let mut mantissa = if exponent == 0 {
            fraction
        } else {
            fraction | (1 << 23)
        } as i128;
        let mut shift = if exponent == 0 { -149 } else { exponent - 150 };
        while mantissa % 2 == 0 {
            mantissa /= 2;
            shift += 1;
        }
        // Out-of-domain values stay unsupported, not silently rounded.
        if !(-120..=100).contains(&shift) {
            return None;
        }
        if bits >> 31 != 0 {
            mantissa = -mantissa;
        }
        Some(if shift >= 0 {
            Self::int(mantissa << shift)
        } else {
            Self::new(mantissa, 1i128 << -shift)
        })
    }
    fn exact_bits(self) -> Option<u32> {
        // A floating conversion proposes a candidate only. Integer-rational
        // equality proves it exact; no floating residual or epsilon is used.
        let candidate = (self.n as f64 / self.d as f64) as f32;
        let bits = candidate.to_bits();
        (Self::from_bits(bits) == Some(self)).then_some(bits)
    }
    fn bits(self) -> Result<u32> {
        self.exact_bits()
            .ok_or_else(|| format!("non-exact float32 rational: {self}"))
    }
}
impl fmt::Display for Rat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.n, self.d)
    }
}
impl Add for Rat {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        let g = gcd(self.d, b.d);
        Self::new(self.n * (b.d / g) + b.n * (self.d / g), self.d * (b.d / g))
    }
}
impl Neg for Rat {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.n, self.d)
    }
}
impl Sub for Rat {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        self + -b
    }
}
impl Mul for Rat {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        let g = gcd(self.n, b.d);
        let h = gcd(b.n, self.d);
        Self::new((self.n / g) * (b.n / h), (self.d / h) * (b.d / g))
    }
}
impl Div for Rat {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        self * Self::new(b.d, b.n)
    }
}
impl PartialOrd for Rat {
    fn partial_cmp(&self, b: &Self) -> Option<std::cmp::Ordering> {
        Some((self.n * b.d).cmp(&(b.n * self.d)))
    }
}

#[derive(Clone, Copy, Debug)]
struct Plane {
    a: Rat,
    x: Rat,
    y: Rat,
}
impl Plane {
    fn constant(a: Rat) -> Self {
        Self {
            a,
            x: Rat::ZERO,
            y: Rat::ZERO,
        }
    }
    fn at(self, x: Rat, y: Rat) -> Rat {
        self.a + self.x * x + self.y * y
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Axis {
    X,
    Y,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Coverage {
    Full,
    Triangle(Axis),
}
#[derive(Clone, Debug)]
struct Case {
    name: String,
    vertices: Vec<[u32; 8]>,
    depth: Plane,
    reciprocal_w: Plane,
    token: u32,
    coverage: Coverage,
    negative: bool,
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn bit(&mut self) -> bool {
        self.next() & 1 != 0
    }
}

fn affine_plane(rng: &mut Rng) -> Plane {
    let (mut x, mut y) = if rng.bit() {
        (Rat::new(1, 32), Rat::new(1, 64))
    } else {
        (Rat::new(1, 64), Rat::new(1, 32))
    };
    if rng.bit() {
        x = -x;
    }
    if rng.bit() {
        y = -y;
    }
    let mut a = if rng.bit() {
        Rat::new(1, 8)
    } else {
        Rat::new(1, 4)
    };
    // Keep every vertex strictly inside the depth clip interval, including
    // after the negative shader adds 1/8 in normalized depth.
    if x < Rat::ZERO {
        a = a - Rat::int(8) * x;
    }
    if y < Rat::ZERO {
        a = a - Rat::int(8) * y;
    }
    Plane { a, x, y }
}

fn make_case(
    name: &str,
    depth: Plane,
    axis: Axis,
    uniform: bool,
    partial: bool,
    negative: bool,
    rng: &mut Rng,
) -> Result<Case> {
    let scale = Rat::pow2(1 - (rng.next() % 4) as i32);
    let reverse = rng.bit();
    let slope = if uniform {
        Rat::ZERO
    } else if reverse {
        scale / Rat::int(16)
    } else {
        -scale / Rat::int(16)
    };
    let reciprocal_w = Plane {
        a: if reverse && !uniform {
            scale / Rat::int(2)
        } else {
            scale
        },
        x: if axis == Axis::X { slope } else { Rat::ZERO },
        y: if axis == Axis::Y { slope } else { Rat::ZERO },
    };
    // Two 8x4 strips, each split along a slope-1/2 diagonal. No pixel center
    // is on any edge, and all projected vertices are inside the clip box.
    let mut triangles = vec![
        [(0, 0), (8, 0), (0, 4)],
        [(8, 0), (8, 4), (0, 4)],
        [(0, 4), (8, 4), (0, 8)],
        [(8, 4), (8, 8), (0, 8)],
    ];
    if partial {
        triangles.truncate(1);
    }
    for i in (1..triangles.len()).rev() {
        let j = rng.next() as usize % (i + 1);
        triangles.swap(i, j);
    }
    let token = (rng.next() as u32) ^ 0x4b19_2a71;
    let mut vertices = Vec::new();
    for mut triangle in triangles {
        triangle.rotate_left(rng.next() as usize % 3);
        if rng.bit() {
            triangle.swap(0, 1);
        }
        for (mut x, mut y) in triangle {
            if axis == Axis::Y {
                std::mem::swap(&mut x, &mut y);
            }
            let sx = Rat::int(x);
            let sy = Rat::int(y);
            let w = Rat::ONE / reciprocal_w.at(sx, sy);
            let z = depth.at(sx, sy) * w;
            let clip_x = (sx / Rat::int(4) - Rat::ONE) * w;
            let clip_y = (Rat::ONE - sy / Rat::int(4)) * w;
            vertices.push([
                clip_x.bits()?,
                clip_y.bits()?,
                z.bits()?,
                w.bits()?,
                vertices.len() as u32,
                rng.next() as u32,
                rng.next() as u32,
                token,
            ]);
        }
    }
    Ok(Case {
        name: name.into(),
        vertices,
        depth,
        reciprocal_w,
        token,
        coverage: if partial {
            Coverage::Triangle(axis)
        } else {
            Coverage::Full
        },
        negative,
    })
}

fn corpus(seed: u64) -> Result<Vec<Case>> {
    let mut rng = Rng(seed);
    let constants = [
        ("constant_0", Rat::ZERO),
        ("constant_2m16", Rat::pow2(-16)),
        ("constant_2m24", Rat::pow2(-24)),
        ("constant_2m32", Rat::pow2(-32)),
        ("constant_2m40", Rat::pow2(-40)),
        ("constant_025", Rat::new(1, 4)),
        ("constant_075", Rat::new(3, 4)),
        ("constant_1", Rat::ONE),
    ];
    let mut cases = Vec::new();
    for (name, d) in constants {
        let axis = if rng.bit() { Axis::X } else { Axis::Y };
        cases.push(make_case(
            name,
            Plane::constant(d),
            axis,
            false,
            false,
            false,
            &mut rng,
        )?);
    }
    for (name, axis, uniform, partial, negative) in [
        ("affine_x", Axis::X, false, false, true),
        ("affine_y", Axis::Y, false, false, true),
        ("affine_uniform_w", Axis::X, true, false, false),
        ("affine_partial", Axis::X, false, true, false),
    ] {
        let plane = affine_plane(&mut rng);
        cases.push(make_case(
            name, plane, axis, uniform, partial, negative, &mut rng,
        )?);
    }
    Ok(cases)
}

fn input_rat(v: &[u32; 8], lane: usize) -> Result<Rat> {
    Rat::from_bits(v[lane]).ok_or_else(|| "input outside exact-rational domain".into())
}
fn projected(v: &[u32; 8]) -> Result<[Rat; 2]> {
    let w = input_rat(v, 3)?;
    if w <= Rat::ZERO {
        return Err("nonpositive clip w is unqualified".into());
    }
    Ok([
        (input_rat(v, 0)? / w + Rat::ONE) * Rat::int(4),
        (Rat::ONE - input_rat(v, 1)? / w) * Rat::int(4),
    ])
}
fn cross(a: [Rat; 2], b: [Rat; 2], p: [Rat; 2]) -> Rat {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
#[derive(Clone, Copy, Debug)]
struct Sample {
    primitive: usize,
    weights: [Rat; 3],
}
fn sample(case: &Case, x: usize, y: usize) -> Result<Option<Sample>> {
    let p = [
        Rat::new((2 * x + 1) as i128, 2),
        Rat::new((2 * y + 1) as i128, 2),
    ];
    let mut hit = None;
    for (primitive, vs) in case.vertices.chunks_exact(3).enumerate() {
        let a = projected(&vs[0])?;
        let b = projected(&vs[1])?;
        let c = projected(&vs[2])?;
        let area = cross(a, b, c);
        if area == Rat::ZERO {
            return Err("degenerate triangle is unqualified".into());
        }
        let weights = [
            cross(b, c, p) / area,
            cross(c, a, p) / area,
            cross(a, b, p) / area,
        ];
        if weights.iter().all(|w| *w >= Rat::ZERO) {
            if weights.contains(&Rat::ZERO) {
                return Err("sample-on-edge rule is unqualified".into());
            }
            if hit.is_some() {
                return Err("overlapping coverage is outside this probe".into());
            }
            hit = Some(Sample { primitive, weights });
        }
    }
    Ok(hit)
}
fn known_coverage(case: &Case, x: usize, y: usize) -> bool {
    match case.coverage {
        Coverage::Full => true,
        Coverage::Triangle(Axis::X) => (2 * x + 1) + 2 * (2 * y + 1) < 16,
        Coverage::Triangle(Axis::Y) => 2 * (2 * x + 1) + (2 * y + 1) < 16,
    }
}
fn incomplete_quad(case: &Case, x: usize, y: usize) -> bool {
    (y & !1..(y & !1) + 2).any(|qy| (x & !1..(x & !1) + 2).any(|qx| !known_coverage(case, qx, qy)))
}
fn models(case: &Case, s: Sample) -> Result<(Rat, Rat, [Rat; 4])> {
    let mut d = Rat::ZERO;
    let mut q = Rat::ZERO;
    let mut perspective_numerator = Rat::ZERO;
    let mut linear_w = Rat::ZERO;
    for i in 0..3 {
        let v = &case.vertices[3 * s.primitive + i];
        let w = input_rat(v, 3)?;
        let ndc = input_rat(v, 2)? / w;
        d = d + s.weights[i] * ndc;
        q = q + s.weights[i] / w;
        perspective_numerator = perspective_numerator + s.weights[i] * ndc / w;
        linear_w = linear_w + s.weights[i] * w;
    }
    Ok((
        d,
        q,
        [perspective_numerator / q, d * q, d / q, d / linear_w],
    ))
}
fn separation(case: &Case) -> Result<[usize; 4]> {
    let mut counts = [0; 4];
    for y in 0..WIDTH {
        for x in 0..WIDTH {
            if let Some(s) = sample(case, x, y)? {
                let (d, _, wrong) = models(case, s)?;
                for i in 0..4 {
                    counts[i] += usize::from(d != wrong[i]);
                }
            }
        }
    }
    Ok(counts)
}

fn initial_output() -> Vec<u32> {
    let mut output = vec![SENTINEL; WORDS];
    output[GUARD..GUARD + 4].fill(0);
    for p in 0..WIDTH * WIDTH {
        output[PIXEL_BASE + p * PWORDS] = 0;
    }
    output
}
fn expected(case: &Case, shift: bool) -> Result<Vec<u32>> {
    if shift && !case.negative {
        return Err("negative variant restricted to interior affine cases".into());
    }
    let mut output = initial_output();
    let delta = if shift { Rat::new(1, 8) } else { Rat::ZERO };
    for (i, v) in case.vertices.iter().enumerate() {
        let xy = projected(v)?;
        let w = input_rat(v, 3)?;
        let z = input_rat(v, 2)?;
        if z < Rat::ZERO || z > w || xy.iter().any(|r| *r < Rat::ZERO || *r > Rat::int(8)) {
            return Err("out-of-clip-box input is unqualified".into());
        }
        if z / w != case.depth.at(xy[0], xy[1])
            || Rat::ONE / w != case.reciprocal_w.at(xy[0], xy[1])
        {
            return Err("constructed vertices disagree with independent planes".into());
        }
        let base = GUARD + HEADER + i * VWORDS;
        output[base] = VERTEX_SEEN;
        output[base + 1] = i as u32;
        output[base + 2..base + 10].copy_from_slice(v);
        output[base + 10..base + 14].copy_from_slice(&v[..4]);
        output[base + 12] = (z + delta * w).bits()?;
    }
    for y in 0..WIDTH {
        for x in 0..WIDTH {
            let hit = sample(case, x, y)?;
            if hit.is_some() != known_coverage(case, x, y) {
                return Err(format!(
                    "{} rational coverage differs from closed-form mask",
                    case.name
                ));
            }
            if let Some(s) = hit {
                let sx = Rat::new((2 * x + 1) as i128, 2);
                let sy = Rat::new((2 * y + 1) as i128, 2);
                let (d, q, _) = models(case, s)?;
                if d != case.depth.at(sx, sy) || q != case.reciprocal_w.at(sx, sy) {
                    return Err("barycentric oracle disagrees with analytic planes".into());
                }
                let base = PIXEL_BASE + (y * WIDTH + x) * PWORDS;
                output[base..base + 11].copy_from_slice(&[
                    1,
                    sx.bits()?,
                    sy.bits()?,
                    (d + delta).bits()?,
                    q.bits()?,
                    s.primitive as u32,
                    case.token,
                    case.depth.x.bits()?,
                    case.depth.y.bits()?,
                    case.reciprocal_w.x.bits()?,
                    case.reciprocal_w.y.bits()?,
                ]);
                output[GUARD + 3] += 1;
            }
        }
    }
    let count = if case.coverage == Coverage::Full {
        64
    } else {
        16
    };
    if output[GUARD + 3] != count {
        return Err("known coverage count failed".into());
    }
    Ok(output)
}

#[derive(Debug)]
struct Difference {
    word: usize,
    expected: u32,
    actual: u32,
}
fn compare(want: &[u32], got: &[u32]) -> Result<Vec<Difference>> {
    if want.len() != WORDS || got.len() != WORDS {
        return Err("readback size mismatch".into());
    }
    Ok(want
        .iter()
        .zip(got)
        .enumerate()
        .filter_map(|(word, (&expected, &actual))| {
            (expected != actual).then_some(Difference {
                word,
                expected,
                actual,
            })
        })
        .collect())
}
fn permitted_depth_change(case: &Case, word: usize) -> bool {
    if (GUARD + HEADER..GUARD + HEADER + case.vertices.len() * VWORDS).contains(&word) {
        return (word - GUARD - HEADER) % VWORDS == 12;
    }
    if (PIXEL_BASE..WORDS - GUARD).contains(&word) {
        let pixel = (word - PIXEL_BASE) / PWORDS;
        return (word - PIXEL_BASE) % PWORDS == 3
            && known_coverage(case, pixel % WIDTH, pixel / WIDTH);
    }
    false
}
fn negative_control(case: &Case, baseline: &[u32], wrong: &[u32]) -> Result<usize> {
    if !compare(&expected(case, false)?, baseline)?.is_empty() {
        return Err("negative control requires a fully qualified baseline".into());
    }
    if !compare(&expected(case, true)?, wrong)?.is_empty() {
        return Err("wrong shader did not match the exact independent shifted plane".into());
    }
    let mut changed_fragment_depths = 0;
    for i in 0..WORDS {
        if !permitted_depth_change(case, i) && baseline[i] != wrong[i] {
            return Err(format!(
                "negative shader changed an unrelated output word {i}"
            ));
        }
    }
    for p in 0..WIDTH * WIDTH {
        if known_coverage(case, p % WIDTH, p / WIDTH) {
            let i = PIXEL_BASE + p * PWORDS + 3;
            if baseline[i] == wrong[i] {
                return Err("wrong shader left a covered depth unchanged".into());
            }
            changed_fragment_depths += 1;
        }
    }
    if changed_fragment_depths != 64 {
        return Err("negative control coverage must be 64".into());
    }
    Ok(changed_fragment_depths)
}
fn component(word: usize) -> &'static str {
    if !(GUARD..WORDS - GUARD).contains(&word) {
        "edge_guard"
    } else if word < GUARD + HEADER {
        "header_or_header_guard"
    } else if word < PIXEL_BASE {
        match (word - GUARD - HEADER) % VWORDS {
            0 | 1 => "vertex_visit",
            2..=9 => "input_echo",
            10..=13 => "emitted_clip",
            _ => "vertex_guard",
        }
    } else {
        match (word - PIXEL_BASE) % PWORDS {
            0 => "fragment_hits",
            1 => "position_x",
            2 => "position_y",
            3 => "position_z",
            4 => "position_reciprocal_w",
            5 => "primitive",
            6 => "runtime_token",
            7..=10 => "derivative",
            _ => "pixel_guard",
        }
    }
}

fn write_file(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path.as_ref(), contents)
        .map_err(|e| format!("write {}: {e}", path.as_ref().display()))
}
fn new_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)
        .map_err(|e| format!("output must be a new directory {}: {e}", path.display()))
}
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                write!(out, "\\u{:04x}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn write_words(path: &Path, words: &[u32]) -> Result<()> {
    let mut text = String::from("word\tcomponent\tu32_hex\n");
    for (i, value) in words.iter().enumerate() {
        writeln!(text, "{i}\t{}\t{value:08x}", component(i)).unwrap();
    }
    write_file(path, text)
}
fn write_differences(path: &Path, differences: &[Difference]) -> Result<()> {
    let mut text = String::from("word\tcomponent\texpected_hex\tactual_hex\n");
    for d in differences {
        writeln!(
            text,
            "{}\t{}\t{:08x}\t{:08x}",
            d.word,
            component(d.word),
            d.expected,
            d.actual
        )
        .unwrap();
    }
    write_file(path, text)
}
fn write_reference(path: &Path, case: &Case) -> Result<()> {
    new_dir(path)?;
    let mut inputs =
        String::from("vertex\tclip_x\tclip_y\tclip_z\tclip_w\ttag0\ttag1\ttag2\ttoken\n");
    for (i, v) in case.vertices.iter().enumerate() {
        write!(inputs, "{i}").unwrap();
        for word in v {
            write!(inputs, "\t{word:08x}").unwrap();
        }
        inputs.push('\n');
    }
    write_file(path.join("input-u32.tsv"), inputs)?;
    let mut planes = String::from("quantity\tconstant\tx_coefficient\ty_coefficient\n");
    for (name, p) in [("depth", case.depth), ("reciprocal_w", case.reciprocal_w)] {
        writeln!(planes, "{name}\t{}\t{}\t{}", p.a, p.x, p.y).unwrap();
    }
    write_file(path.join("planes.tsv"), planes)?;
    write_words(&path.join("expected-u32.tsv"), &expected(case, false)?)?;
    let mut table = String::from("x\ty\tcovered\tprimitive\tuncovered_quad_neighbor\tz_exact\treciprocal_w_exact\twrong_perspective_ndc\twrong_second_divide\twrong_divide_by_reciprocal_w\twrong_divide_by_linear_w\n");
    for y in 0..WIDTH {
        for x in 0..WIDTH {
            if let Some(s) = sample(case, x, y)? {
                let (d, q, bad) = models(case, s)?;
                writeln!(
                    table,
                    "{x}\t{y}\t1\t{}\t{}\t{d}\t{q}\t{}\t{}\t{}\t{}",
                    s.primitive,
                    incomplete_quad(case, x, y),
                    bad[0],
                    bad[1],
                    bad[2],
                    bad[3]
                )
                .unwrap();
            } else {
                writeln!(table, "{x}\t{y}\t0\t-\t-\t-\t-\t-\t-\t-\t-").unwrap();
            }
        }
    }
    write_file(path.join("models.tsv"), table)
}

fn portable_reference(path: &Path, seed: u64) -> Result<()> {
    new_dir(path)?;
    let cases = corpus(seed)?;
    let mut controls = 0;
    for case in &cases {
        let dir = path.join(&case.name);
        write_reference(&dir, case)?;
        let reference = expected(case, false)?;
        let differences = compare(&reference, &reference)?;
        write_differences(&dir.join("synthetic-self-check.tsv"), &differences)?;
        if case.negative {
            if separation(case)?.contains(&0) {
                return Err("wrong models not separated".into());
            }
            negative_control(case, &reference, &expected(case, true)?)?;
            controls += 1;
        }
    }
    write_file(path.join("summary.json"), format!(
        "{{\"schema\":1,\"kind\":\"portable_reference_only\",\"native_executed\":false,\"seed\":\"{seed:016x}\",\"cases\":{},\"synthetic_negative_controls\":{controls},\"qualification\":\"none\"}}\n", cases.len()))?;
    println!("PORTABLE ONLY: {} fresh generated cases, {controls} synthetic negative controls; no GPU claim", cases.len());
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Receipt {
    abi: u32,
    completed: u32,
    status: u32,
    no_error: u32,
    copied: u32,
    variant: u32,
    vertices: u32,
    words: u32,
    no_depth_stencil: u32,
    default_viewport_scissor: u32,
    samples: u32,
    input_unchanged: u32,
    registry_id: u64,
    reserved: u64,
}
fn receipt_ok(r: Receipt, case: &Case, variant: u32) -> bool {
    r.abi == 1
        && r.completed == 1
        && r.status == 4
        && r.no_error == 1
        && r.copied == 1
        && r.variant == variant
        && r.vertices == case.vertices.len() as u32
        && r.words == WORDS as u32
        && r.no_depth_stencil == 1
        && r.default_viewport_scissor == 1
        && r.samples == 1
        && r.input_unchanged == 1
        && r.reserved == 0
}
#[cfg(all(depth_native, not(target_os = "macos")))]
compile_error!("depth_native is macOS-only; no emulated native path is permitted");
#[cfg(depth_native)]
extern "C" {
    fn depth_native(
        source: *const std::ffi::c_char,
        input: *const u32,
        count: u32,
        variant: u32,
        output: *mut u32,
        words: usize,
        receipt: *mut Receipt,
        json: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}
// The complete Rust observation/comparison path is typechecked in portable
// builds too. Only this thin platform call is conditional; its portable branch
// refuses execution and never writes synthetic output.
fn invoke_native(
    source: &std::ffi::CStr,
    case: &Case,
    variant: u32,
    output: &mut [u32],
    receipt: &mut Receipt,
    json: &mut [u8],
) -> Result<i32> {
    #[cfg(depth_native)]
    {
        Ok(unsafe {
            depth_native(
                source.as_ptr(),
                case.vertices.as_ptr().cast(),
                case.vertices.len() as u32,
                variant,
                output.as_mut_ptr(),
                output.len(),
                receipt,
                json.as_mut_ptr().cast(),
                json.len(),
            )
        })
    }
    #[cfg(not(depth_native))]
    {
        let _ = (source, case, variant, output, receipt, json);
        Err("native backend not built; no CPU-as-GPU fallback".into())
    }
}
struct Observation {
    words: Vec<u32>,
    device: u64,
}
fn native_case(path: &Path, case: &Case, variant: u32) -> Result<Observation> {
    let source = std::ffi::CString::new(SHADER).map_err(|e| e.to_string())?;
    let mut output = initial_output();
    let mut receipt = Receipt::default();
    let mut json = vec![0u8; 32768];
    // The native bridge copies only a completed GPU buffer into this array.
    // Neither expected() nor any synthetic data is used to populate readback.
    let code = invoke_native(&source, case, variant, &mut output, &mut receipt, &mut json)?;
    let end = json.iter().position(|b| *b == 0).unwrap_or(json.len());
    write_file(
        path.join("native.json"),
        if end == 0 {
            b"{\"metadata_missing\":true}\n".as_slice()
        } else {
            &json[..end]
        },
    )?;
    write_file(
        path.join("receipt.txt"),
        format!("bridge_return={code}\n{receipt:#?}\n"),
    )?;
    if receipt.copied == 1 {
        write_words(&path.join("observed-u32.tsv"), &output)?;
    }
    if code != 0 || !receipt_ok(receipt, case, variant) {
        return Err(format!(
            "{} variant {variant}: native receipt failed (code {code}); no qualification",
            case.name
        ));
    }
    Ok(Observation {
        words: output,
        device: receipt.registry_id,
    })
}
fn reserve_draw(calls: &mut usize) -> Result<()> {
    if *calls >= 14 {
        return Err("hard fourteen-draw budget exhausted".into());
    }
    *calls += 1;
    Ok(())
}
fn require_same_device(anchor: &mut Option<u64>, device: u64) -> Result<()> {
    if anchor.is_some_and(|id| id != device) {
        return Err("default Metal device changed within the experiment".into());
    }
    *anchor = Some(device);
    Ok(())
}
fn write_comparison_summary(path: &Path, differences: &[Difference]) -> Result<()> {
    let mut counts = std::collections::BTreeMap::new();
    for d in differences {
        *counts.entry(component(d.word)).or_insert(0usize) += 1;
    }
    let fields = counts
        .iter()
        .map(|(name, count)| format!("{}:{count}", json_string(name)))
        .collect::<Vec<_>>()
        .join(",");
    write_file(path, format!(
        "{{\"schema\":1,\"kind\":\"completed_gpu_comparison\",\"strict_bits\":true,\"all_words_equal\":{},\"mismatched_words\":{},\"mismatches_by_component\":{{{fields}}}}}\n",
        differences.is_empty(), differences.len()))
}
fn observe_body(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut seed_bytes = [0; 8];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed_bytes))
        .map_err(|e| e.to_string())?;
    let seed = u64::from_ne_bytes(seed_bytes);
    new_dir(path)?;
    write_file(path.join("run.json"), format!("{{\"schema\":1,\"kind\":\"native_observation\",\"seed\":\"{seed:016x}\",\"seed_source\":\"os_random\",\"maximum_draws\":14}}\n"))?;
    let mut calls = 0;
    let work =
        (|| -> Result<()> {
            let cases = corpus(seed)?;
            let mut baselines = Vec::new();
            let mut failed = Vec::new();
            let mut device = None;
            for case in &cases {
                let dir = path.join(&case.name);
                write_reference(&dir, case)?;
                reserve_draw(&mut calls)?;
                let actual = native_case(&dir, case, 0)?;
                require_same_device(&mut device, actual.device)?;
                let differences = compare(&expected(case, false)?, &actual.words)?;
                write_differences(&dir.join("comparison.tsv"), &differences)?;
                write_comparison_summary(&dir.join("comparison.json"), &differences)?;
                println!("{}: {} exact-word mismatches", case.name, differences.len());
                if !differences.is_empty() {
                    failed.push(case.name.clone());
                }
                baselines.push(actual.words);
            }
            for (case, baseline) in cases.iter().zip(&baselines).filter(|(c, _)| c.negative) {
                if separation(case)?.contains(&0) {
                    return Err("corpus does not distinguish every wrong model".into());
                }
                let dir = path.join(format!("{}_wrong_shader", case.name));
                write_reference(&dir, case)?;
                reserve_draw(&mut calls)?;
                let wrong = native_case(&dir, case, 1)?;
                require_same_device(&mut device, wrong.device)?;
                let rejected = compare(&expected(case, false)?, &wrong.words)?;
                let shifted = compare(&expected(case, true)?, &wrong.words)?;
                write_differences(&dir.join("original-plane-rejection.tsv"), &rejected)?;
                write_differences(&dir.join("shifted-plane-comparison.tsv"), &shifted)?;
                write_comparison_summary(&dir.join("shifted-plane-comparison.json"), &shifted)?;
                let control = negative_control(case, baseline, &wrong.words);
                write_file(
                    dir.join("control.json"),
                    format!(
                "{{\"schema\":1,\"passed\":{},\"changed_fragment_depths\":{},\"error\":{}}}\n",
                control.is_ok(), control.as_ref().copied().unwrap_or(0),
                control.as_ref().err().map(|s| json_string(s)).unwrap_or_else(|| "null".into())),
                )?;
                if control.is_err() {
                    failed.push(format!("{}_negative_control", case.name));
                }
            }
            if !failed.is_empty() {
                return Err(format!(
                    "unqualified exact observations: {}",
                    failed.join(", ")
                ));
            }
            Ok(())
        })();
    write_file(path.join("qualification.json"), format!(
        "{{\"schema\":1,\"native_draw_calls_attempted\":{calls},\"bounded_cases_qualified\":{},\"arbitrary_rounding_or_clipping\":\"unknown\",\"direct_helper_outputs\":\"unknown\",\"error\":{}}}\n",
        work.is_ok(), work.as_ref().err().map(|s| json_string(s)).unwrap_or_else(|| "null".into())))?;
    work
}
fn observe(path: &Path) -> Result<()> {
    if cfg!(depth_native) {
        observe_body(path)
    } else {
        Err("native backend not built; no CPU-as-GPU fallback".into())
    }
}

fn main_result() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    // Source and JSON escaping are part of the portable package as well,
    // without implying a native compile happened.
    if SHADER.is_empty() || json_string("") != "\"\"" {
        return Err("source contract failure".into());
    }
    match args.get(1).map(String::as_str) {
        Some("reference") if args.len() == 3 || args.len() == 4 => {
            let seed = if let Some(s) = args.get(3) {
                u64::from_str_radix(s, 16).map_err(|_| "seed must be 1-16 hexadecimal digits")?
            } else {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?;
                t.as_nanos() as u64 ^ (u64::from(std::process::id()) << 32)
            };
            portable_reference(Path::new(&args[2]), seed)
        }
        Some("observe") if args.len() == 3 => observe(Path::new(&args[2])),
        _ => Err(
            "usage: probe reference NEW_DIRECTORY [HEX_SEED] | probe observe NEW_DIRECTORY".into(),
        ),
    }
}
fn main() {
    if let Err(error) = main_result() {
        eprintln!("UNQUALIFIED: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cases() -> Vec<Case> {
        corpus(0x681927d452eca301).unwrap()
    }
    #[test]
    fn exact_rational_float_candidates_are_verified() {
        for e in [-40, -32, -24, -16, -2, 0, 4] {
            let r = Rat::pow2(e);
            assert_eq!(Rat::from_bits(r.bits().unwrap()), Some(r));
        }
        assert_eq!(Rat::new(1, 3).exact_bits(), None);
        assert_eq!(Rat::from_bits(f32::NAN.to_bits()), None);
        assert_eq!(Rat::from_bits(f32::INFINITY.to_bits()), None);
        assert_eq!(Rat::from_bits(1), None); // unsupported subnormal, not zero
        assert_eq!(Rat::ZERO.bits().unwrap(), 0);
        assert_eq!((Rat::new(1, 3) - Rat::new(2, 3)).abs(), Rat::new(1, 3));
    }
    #[test]
    fn corpus_is_bounded_and_freshly_seeded() {
        let a = cases();
        let b = corpus(2).unwrap();
        assert_eq!(a.len(), 12);
        assert_eq!(a.iter().filter(|c| c.negative).count(), 2);
        assert!(a.iter().zip(b).all(|(x, y)| x.vertices != y.vertices));
        assert!(a
            .iter()
            .all(|c| c.vertices.len() == 3 || c.vertices.len() == 12));
    }
    #[test]
    fn all_seeded_planes_cross_check_independent_barycentrics() {
        for seed in 0..32 {
            for c in corpus(seed).unwrap() {
                expected(&c, false).unwrap();
            }
        }
    }
    #[test]
    fn exact_small_normals_survive_without_quantization() {
        for (i, exponent) in [(1, -16), (2, -24), (3, -32), (4, -40)] {
            let c = &cases()[i];
            let e = expected(c, false).unwrap();
            for p in 0..64 {
                let bits = e[PIXEL_BASE + p * PWORDS + 3];
                assert_eq!(bits, Rat::pow2(exponent).bits().unwrap());
                assert_ne!((bits >> 23) & 255, 0);
            }
        }
    }
    #[test]
    fn full_and_partial_masks_have_known_counts_without_edge_ties() {
        for c in cases() {
            let e = expected(&c, false).unwrap();
            assert_eq!(
                e[GUARD + 3],
                if c.coverage == Coverage::Full { 64 } else { 16 }
            );
            for y in 0..8 {
                for x in 0..8 {
                    assert_eq!(
                        sample(&c, x, y).unwrap().is_some(),
                        known_coverage(&c, x, y)
                    );
                }
            }
        }
    }
    #[test]
    fn partial_case_contains_incomplete_quad_witnesses() {
        let c = &cases()[11];
        let witnesses = (0..64)
            .filter(|p| known_coverage(c, p % 8, p / 8) && incomplete_quad(c, p % 8, p / 8))
            .count();
        assert_eq!(witnesses, 8);
        let quads: std::collections::BTreeSet<_> = (0..64)
            .filter(|p| known_coverage(c, p % 8, p / 8) && incomplete_quad(c, p % 8, p / 8))
            .map(|p| ((p % 8) / 2, (p / 8) / 2))
            .collect();
        assert_eq!(quads.len(), 4);
        assert_ne!(c.depth.x, Rat::ZERO);
        assert_ne!(c.depth.y, Rat::ZERO);
    }
    #[test]
    fn all_wrong_depth_models_are_rationally_distinguishable() {
        for seed in 0..32 {
            for c in corpus(seed).unwrap().iter().filter(|c| c.negative) {
                let counts = separation(c).unwrap();
                assert!(
                    counts.iter().all(|n| *n >= 32),
                    "{seed} {} {counts:?}",
                    c.name
                );
            }
        }
    }
    #[test]
    fn hand_derived_binary_rational_witness_checks_the_oracle() {
        // Newly derived analytic example, never a native input fixture.
        let rows = [
            [-1.0f32, 1.0, 0.125, 1.0],
            [2.0, 2.0, 0.75, 2.0],
            [-1.0, 0.0, 0.1875, 1.0],
        ];
        let c = Case {
            name: "analytic_test_only".into(),
            vertices: rows
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    [
                        p[0].to_bits(),
                        p[1].to_bits(),
                        p[2].to_bits(),
                        p[3].to_bits(),
                        i as u32,
                        0,
                        0,
                        1,
                    ]
                })
                .collect(),
            depth: Plane {
                a: Rat::new(1, 8),
                x: Rat::new(1, 32),
                y: Rat::new(1, 64),
            },
            reciprocal_w: Plane {
                a: Rat::ONE,
                x: Rat::new(-1, 16),
                y: Rat::ZERO,
            },
            token: 1,
            coverage: Coverage::Triangle(Axis::X),
            negative: false,
        };
        let s = sample(&c, 0, 0).unwrap().unwrap();
        assert_eq!(
            s.weights,
            [Rat::new(13, 16), Rat::new(1, 16), Rat::new(1, 8)]
        );
        let (d, q, alternatives) = models(&c, s).unwrap();
        assert_eq!(d, Rat::new(19, 128));
        assert_eq!(q, Rat::new(31, 32));
        assert_eq!(
            alternatives,
            [
                Rat::new(35, 248),
                Rat::new(589, 4096),
                Rat::new(19, 124),
                Rat::new(19, 136),
            ]
        );
        expected(&c, false).unwrap();
    }
    #[test]
    fn reciprocal_w_is_the_independent_affine_plane() {
        for c in cases().iter().filter(|c| c.negative) {
            let e = expected(c, false).unwrap();
            let values: std::collections::BTreeSet<_> =
                (0..64).map(|p| e[PIXEL_BASE + p * PWORDS + 4]).collect();
            assert_eq!(values.len(), 8);
        }
    }
    #[test]
    fn synthetic_reference_is_self_consistent_only() {
        for c in cases() {
            let e = expected(&c, false).unwrap();
            assert!(compare(&e, &e).unwrap().is_empty());
        }
    }
    #[test]
    fn untouched_readback_cannot_pass() {
        for c in cases() {
            assert!(!compare(&expected(&c, false).unwrap(), &initial_output())
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn every_guard_region_is_checked() {
        let c = &cases()[0];
        let e = expected(c, false).unwrap();
        for i in [
            0,
            GUARD - 1,
            GUARD + 4,
            GUARD + HEADER + 14,
            PIXEL_BASE + 11,
            WORDS - GUARD,
            WORDS - 1,
        ] {
            let mut got = e.clone();
            got[i] ^= 1;
            let d = compare(&e, &got).unwrap();
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].word, i);
        }
    }
    #[test]
    fn every_vertex_input_lane_and_tag_must_echo_exactly() {
        let c = &cases()[0];
        let e = expected(c, false).unwrap();
        for v in 0..c.vertices.len() {
            for lane in 0..8 {
                let mut got = e.clone();
                got[GUARD + HEADER + v * VWORDS + 2 + lane] ^= 1;
                assert_eq!(compare(&e, &got).unwrap().len(), 1);
            }
        }
    }
    #[test]
    fn missing_duplicate_wrong_primitive_and_uncovered_writes_fail() {
        let c = &cases()[11];
        let e = expected(c, false).unwrap();
        for i in [
            GUARD + 2,
            GUARD + 3,
            PIXEL_BASE,
            PIXEL_BASE + 5,
            PIXEL_BASE + 6,
            PIXEL_BASE + 63 * PWORDS,
            PIXEL_BASE + 63 * PWORDS + 3,
        ] {
            let mut got = e.clone();
            got[i] ^= 1;
            assert!(!compare(&e, &got).unwrap().is_empty());
        }
    }
    #[test]
    fn z_one_bit_is_not_hidden_by_an_epsilon() {
        let c = &cases()[8];
        let e = expected(c, false).unwrap();
        let mut got = e.clone();
        got[PIXEL_BASE + 3] ^= 1;
        assert_eq!(compare(&e, &got).unwrap().len(), 1);
    }
    #[test]
    fn synthetic_shift_control_requires_all_unrelated_words_unchanged() {
        for c in cases().iter().filter(|c| c.negative) {
            let base = expected(c, false).unwrap();
            let shifted = expected(c, true).unwrap();
            assert_eq!(negative_control(c, &base, &shifted).unwrap(), 64);
            assert_eq!(
                compare(&base, &shifted).unwrap().len(),
                64 + c.vertices.len()
            );
            assert!(negative_control(c, &base, &base).is_err());
            for i in [
                0,
                GUARD + 3,
                GUARD + HEADER + 2,
                PIXEL_BASE + 1,
                PIXEL_BASE + 2,
                PIXEL_BASE + 4,
                PIXEL_BASE + 7,
                PIXEL_BASE + 10,
            ] {
                let mut bad = shifted.clone();
                bad[i] ^= 1;
                assert!(negative_control(c, &base, &bad).is_err());
            }
        }
    }
    #[test]
    fn invalid_baseline_cannot_validate_negative_control() {
        let c = &cases()[8];
        let mut base = expected(c, false).unwrap();
        base[PIXEL_BASE + 4] ^= 1;
        assert!(negative_control(c, &base, &expected(c, true).unwrap()).is_err());
        assert!(expected(&cases()[7], true).is_err());
    }
    #[test]
    fn completion_and_render_contract_flags_are_all_required() {
        let c = &cases()[8];
        let good = Receipt {
            abi: 1,
            completed: 1,
            status: 4,
            no_error: 1,
            copied: 1,
            variant: 0,
            vertices: 12,
            words: WORDS as u32,
            no_depth_stencil: 1,
            default_viewport_scissor: 1,
            samples: 1,
            input_unchanged: 1,
            registry_id: 1,
            reserved: 0,
        };
        assert_eq!(std::mem::size_of::<Receipt>(), 64);
        assert!(receipt_ok(good, c, 0));
        let mutations: [fn(&mut Receipt); 13] = [
            |r| r.abi = 0,
            |r| r.completed = 0,
            |r| r.status = 5,
            |r| r.no_error = 0,
            |r| r.copied = 0,
            |r| r.variant = 1,
            |r| r.vertices = 3,
            |r| r.words -= 1,
            |r| r.no_depth_stencil = 0,
            |r| r.default_viewport_scissor = 0,
            |r| r.samples = 4,
            |r| r.input_unchanged = 0,
            |r| r.reserved = 1,
        ];
        for mutate in mutations {
            let mut bad = good;
            mutate(&mut bad);
            assert!(!receipt_ok(bad, c, 0));
        }
    }
    #[test]
    fn source_contract_is_raw_position_and_a_real_vertex_perturbation() {
        assert!(SHADER.contains("float4 p = in.position;"));
        assert!(SHADER.contains("uint4 position_bits = as_type<uint4>(p);"));
        assert!(SHADER.contains("result.position.z += result.position.w * 0.125f;"));
        assert!(SHADER.contains("dfdx(p.z), dfdy(p.z), dfdx(p.w), dfdy(p.w)"));
        assert!(!SHADER.contains("half"));
        assert!(!SHADER.contains("[[depth"));
        assert!(
            SHADER.find("float4 derivatives").unwrap() < SHADER.find("if (!all(isfinite").unwrap()
        );
    }
    #[test]
    fn bridge_and_word_layout_contract_is_bounded() {
        let bridge = include_str!("bridge.m");
        assert_eq!(WORDS, 1360);
        for text in [
            "OUTPUT_WORDS = 1360",
            "GUARD_WORDS = 64",
            "@\"DEPTH_SHIFT\": @(variant)",
            "MTLResourceStorageModeShared",
            "MTLPixelFormatRGBA32Float",
            "20 * NSEC_PER_SEC",
        ] {
            assert!(bridge.contains(text), "{text}");
        }
        for text in [
            "setViewport:",
            "setViewports:",
            "setScissorRect:",
            "setScissorRects:",
            "setDepthStencilState:",
        ] {
            assert!(!bridge.contains(text), "forbidden override {text}");
        }
        assert!(
            bridge.find("[command waitUntilCompleted]").unwrap()
                < bridge.find("memcpy(output, audit.contents").unwrap()
        );
    }
    #[test]
    fn json_escaping_is_not_path_or_message_injection() {
        assert_eq!(
            json_string("a\"b\\c\n\r\t\u{1}"),
            "\"a\\\"b\\\\c\\n\\r\\t\\u0001\""
        );
    }
    #[test]
    fn non_native_observe_fails_closed() {
        #[cfg(not(depth_native))]
        assert!(observe(Path::new("unused-output-path"))
            .unwrap_err()
            .contains("no CPU-as-GPU fallback"));
    }
    #[test]
    fn hard_draw_budget_cannot_be_exceeded() {
        let mut calls = 0;
        for _ in 0..14 {
            reserve_draw(&mut calls).unwrap();
        }
        assert!(reserve_draw(&mut calls).is_err());
        assert_eq!(calls, 14);
    }
    #[test]
    fn changing_devices_invalidates_a_control_cohort() {
        let mut device = None;
        require_same_device(&mut device, 42).unwrap();
        require_same_device(&mut device, 42).unwrap();
        assert!(require_same_device(&mut device, 43).is_err());
        assert_eq!(device, Some(42));
    }
    #[test]
    fn absent_native_dispatch_cannot_mutate_an_output_or_receipt() {
        #[cfg(not(depth_native))]
        {
            let c = &cases()[8];
            let source = std::ffi::CString::new(SHADER).unwrap();
            let mut output = initial_output();
            let before = output.clone();
            let mut receipt = Receipt::default();
            let mut json = [0; 16];
            assert!(invoke_native(&source, c, 0, &mut output, &mut receipt, &mut json).is_err());
            assert_eq!(output, before);
            assert_eq!(receipt.copied, 0);
            assert_eq!(json, [0; 16]);
        }
    }
}
