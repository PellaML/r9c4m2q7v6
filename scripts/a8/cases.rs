// Public, synthetic scalar-texture inputs. No dependencies and no native interop.
#![forbid(unsafe_code)]

use std::{env, fs, io, path::Path};

const MAX_ROWS: usize = 8192;

#[derive(Clone)]
struct Group {
    name: String,
    start: usize,
    count: usize,
}

struct Case {
    name: &'static str,
    width: usize,
    height: usize,
    seed: Vec<u8>,
    uv: Vec<[u32; 2]>,
    groups: Vec<Group>,
}

// Only finite inputs are used. Preserve signed zero in the edge cases.
fn next_down(x: f32) -> f32 {
    if x == 0.0 {
        return -f32::from_bits(1);
    }
    f32::from_bits(if x > 0.0 {
        x.to_bits() - 1
    } else {
        x.to_bits() + 1
    })
}

fn next_up(x: f32) -> f32 {
    if x == 0.0 {
        return f32::from_bits(1);
    }
    f32::from_bits(if x > 0.0 {
        x.to_bits() + 1
    } else {
        x.to_bits() - 1
    })
}

fn neighbors(x: f32) -> [f32; 3] {
    [next_down(x), x, next_up(x)]
}

// Define the rational in binary64, then round ONCE to binary32. Neighbor
// queries are adjacent representable *normalized coordinates*, not offsets
// computed in the shader. The serialized u32 words are authoritative.
fn coord(base: usize, phase: f64, dimension: usize) -> f32 {
    ((base as f64 + 0.5 + phase) / dimension as f64) as f32
}

impl Case {
    fn new(name: &'static str, width: usize, height: usize, seed: Vec<u8>) -> Self {
        assert_eq!(seed.len(), width * height);
        Self {
            name,
            width,
            height,
            seed,
            uv: Vec::new(),
            groups: Vec::new(),
        }
    }

    fn push(&mut self, u: f32, v: f32) {
        assert!(u.is_finite() && v.is_finite());
        self.uv.push([u.to_bits(), v.to_bits()]);
    }

    fn group(&mut self, name: impl Into<String>, f: impl FnOnce(&mut Self)) {
        let start = self.uv.len();
        f(self);
        self.groups.push(Group {
            name: name.into(),
            start,
            count: self.uv.len() - start,
        });
    }

    fn centers(&mut self) {
        self.group("centers-row-major", |s| {
            for y in 0..s.height {
                for x in 0..s.width {
                    s.push(coord(x, 0.0, s.width), coord(y, 0.0, s.height));
                }
            }
        });
    }

    fn axis(&mut self, vertical: bool) {
        let dimension = if vertical { self.height } else { self.width };
        if dimension == 1 {
            return;
        }
        let base = if dimension > 2 {
            (dimension - 2) / 2
        } else {
            0
        };
        let fixed = if vertical {
            coord(self.width / 2, 0.0, self.width)
        } else {
            coord(self.height / 2, 0.0, self.height)
        };
        let axis = if vertical { "y" } else { "x" };
        self.group(format!("{axis}-base{base}-phases-0-through-256"), |s| {
            for k in 0..=256 {
                let t = coord(base, k as f64 / 256.0, dimension);
                if vertical {
                    s.push(fixed, t);
                } else {
                    s.push(t, fixed);
                }
            }
        });
        self.group(format!("{axis}-base{base}-half-step-ulp-triplets"), |s| {
            for k in 0..256 {
                let t = coord(base, (k as f64 + 0.5) / 256.0, dimension);
                for n in neighbors(t) {
                    if vertical {
                        s.push(fixed, n);
                    } else {
                        s.push(n, fixed);
                    }
                }
            }
        });
    }

    fn corners(&mut self) {
        if self.width == 1 || self.height == 1 {
            return;
        }
        let mut bases = vec![
            (0, 0),
            (self.width - 2, 0),
            (0, self.height - 2),
            (self.width - 2, self.height - 2),
        ];
        bases.sort_unstable();
        bases.dedup();
        for (x, y) in bases {
            self.group(format!("cell-{x}-{y}-selected-2d-phases"), |s| {
                for ky in [0, 1, 63, 127, 128, 129, 255, 256] {
                    for kx in [0, 1, 63, 127, 128, 129, 255, 256] {
                        s.push(
                            coord(x, kx as f64 / 256.0, s.width),
                            coord(y, ky as f64 / 256.0, s.height),
                        );
                    }
                }
            });
            self.group(
                format!("cell-{x}-{y}-selected-2d-half-step-neighbors"),
                |s| {
                    for (kx, ky) in [
                        (0, 255),
                        (1, 127),
                        (63, 191),
                        (127, 128),
                        (128, 127),
                        (191, 63),
                        (254, 1),
                        (255, 0),
                    ] {
                        for v in neighbors(coord(y, (ky as f64 + 0.5) / 256.0, s.height)) {
                            for u in neighbors(coord(x, (kx as f64 + 0.5) / 256.0, s.width)) {
                                s.push(u, v);
                            }
                        }
                    }
                },
            );
        }
    }

    fn edges(&mut self) {
        fn positions(n: usize) -> Vec<f32> {
            let mut p = vec![-2.0, -1.0, -0.0, 0.0, f32::from_bits(1)];
            p.extend(neighbors(coord(0, 0.0, n)));
            p.extend(neighbors(coord(n - 1, 0.0, n)));
            p.extend(neighbors(1.0));
            p.push(2.0);
            p.dedup_by_key(|v| v.to_bits());
            p
        }
        self.group("clamp-edges-and-corners", |s| {
            for u in positions(s.width) {
                for v in [-1.0, 0.5, 2.0] {
                    s.push(u, v);
                }
            }
            for v in positions(s.height) {
                for u in [-1.0, 0.5, 2.0] {
                    s.push(u, v);
                }
            }
        });
    }
}

fn cases() -> Vec<Case> {
    let mut all = vec![
        Case::new("ramp", 16, 16, (0..=255).collect()),
        Case::new("tile2", 2, 2, vec![0, 173, 61, 255]),
        Case::new(
            "tile7",
            7,
            3,
            vec![
                0, 191, 23, 255, 67, 149, 5, 221, 41, 113, 7, 239, 89, 157, 31, 251, 79, 131, 3,
                199, 53,
            ],
        ),
        Case::new("lineh", 7, 1, vec![255, 13, 181, 0, 97, 241, 53]),
        Case::new("linev", 1, 7, vec![0, 197, 31, 255, 83, 149, 5]),
    ];
    for c in &mut all {
        c.centers();
        if c.name != "ramp" {
            c.axis(false);
            c.axis(true);
            c.corners();
            c.edges();
        }
    }
    assert!(all.iter().map(|c| c.uv.len()).sum::<usize>() <= MAX_ROWS);
    all
}

fn uv_bytes(c: &Case) -> Vec<u8> {
    c.uv.iter()
        .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
        .collect()
}

fn write_inputs(out: &Path) -> io::Result<()> {
    // The runner supplies a fresh directory; refuse to overwrite any input.
    let all = cases();
    let rows: usize = all.iter().map(|c| c.uv.len()).sum();
    let mut entries = Vec::new();
    for c in &all {
        let seed = format!("{}-seed.bin", c.name);
        let uv = format!("{}-uv.bin", c.name);
        for (name, bytes) in [(&seed, c.seed.clone()), (&uv, uv_bytes(c))] {
            use std::io::Write;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(out.join(name))?
                .write_all(&bytes)?;
        }
        let groups = c
            .groups
            .iter()
            .map(|g| {
                format!(
                    "{{\"name\":\"{}\",\"start\":{},\"count\":{}}}",
                    g.name, g.start, g.count
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        entries.push(format!(
            "{{\"name\":\"{}\",\"width\":{},\"height\":{},\"centerCount\":{},\"rowCount\":{},\"seedFile\":\"{}\",\"uvFile\":\"{}\",\"groups\":[{}]}}",
            c.name, c.width, c.height, c.seed.len(), c.uv.len(), seed, uv, groups
        ));
    }
    let json = format!(concat!(
        "{{\n\"schema\":1,\n\"totalRows\":{},\n",
        "\"seedLayout\":\"one byte per texel, tightly packed row-major\",\n",
        "\"coordinateLayout\":\"little-endian u32x2 containing exact binary32 normalized u,v bits\",\n",
        "\"coordinateConstruction\":\"round rational normalized coordinate once to f32; half-step triplets are nextDown, exact, nextUp\",\n",
        "\"cases\":[{}]\n}}\n"), rows, entries.join(",\n"));
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("cases.json"))?
        .write_all(json.as_bytes())?;
    println!("synthetic cases={} coordinate_rows={rows}", all.len());
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected output directory",
        ));
    }
    write_inputs(Path::new(&args[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_and_deterministic() {
        let a = cases();
        let b = cases();
        let total: usize = a.iter().map(|c| c.uv.len()).sum();
        assert!((7000..=MAX_ROWS).contains(&total));
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.seed.len(), x.width * x.height);
            assert_eq!(uv_bytes(x), uv_bytes(y));
            assert_eq!(uv_bytes(x).len(), x.uv.len() * 8);
            let mut offset = 0;
            for g in &x.groups {
                assert_eq!(g.start, offset);
                offset += g.count;
            }
            assert_eq!(offset, x.uv.len());
        }
    }

    #[test]
    fn ramp_and_exact_centers() {
        let a = cases();
        assert_eq!(a[0].seed, (0..=255).collect::<Vec<u8>>());
        for c in a {
            assert_eq!(c.groups[0].count, c.seed.len());
            for (i, p) in c.uv.iter().take(c.seed.len()).enumerate() {
                assert_eq!(p[0], coord(i % c.width, 0.0, c.width).to_bits());
                assert_eq!(p[1], coord(i / c.width, 0.0, c.height).to_bits());
            }
        }
    }

    #[test]
    fn phase_counts_and_adjacent_float_triplets() {
        for c in cases() {
            for g in &c.groups {
                if g.name.ends_with("phases-0-through-256") {
                    assert_eq!(g.count, 257);
                }
                if g.name.ends_with("half-step-ulp-triplets") {
                    assert_eq!(g.count, 768);
                    let axis = usize::from(g.name.starts_with('y'));
                    for p in c.uv[g.start..g.start + g.count].chunks_exact(3) {
                        assert_eq!(p[0][axis] + 1, p[1][axis]);
                        assert_eq!(p[1][axis] + 1, p[2][axis]);
                    }
                }
            }
        }
    }

    #[test]
    fn neighbors_preserve_zero_edges_and_order() {
        assert_eq!(next_down(0.0).to_bits(), 0x8000_0001);
        assert_eq!(next_up(-0.0).to_bits(), 1);
        for x in [-2.0, -1.0, 0.25, 0.5, 1.0, 2.0] {
            assert!(next_down(x) < x && next_up(x) > x);
        }
        for c in cases().into_iter().skip(1) {
            assert!(c.uv.iter().any(|p| p.contains(&0x8000_0000)));
            assert!(c.uv.iter().any(|p| p.contains(&(-2.0f32).to_bits())));
        }
    }
}
