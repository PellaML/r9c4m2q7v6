use std::collections::BTreeSet;

pub const CENTER: [u32; 3] = [0x3f00_0000; 3];
pub const GRID_WEIGHTS: [i32; 15] = [
    -1, 0, 1, 127, 128, 129, 255, 256, 257, 383, 384, 385, 511, 512, 513,
];

pub struct Cohort {
    pub coordinates: Vec<[u32; 3]>,
    pub core_count: usize,
}

struct Builder {
    seen: BTreeSet<[u32; 3]>,
    coordinates: Vec<[u32; 3]>,
}

impl Builder {
    fn add(&mut self, c: [f32; 3]) {
        assert!(c.iter().all(|v| v.is_finite()));
        let bits = c.map(f32::to_bits);
        if self.seen.insert(bits) {
            self.coordinates.push(bits);
        }
    }
    fn sweep(&mut self, x: f32) {
        self.add([x, 0.5, 0.5]);
        self.add([0.5, x, 0.5]);
        self.add([0.5, 0.5, x]);
        self.add([x, x, x]);
    }
}

fn above(x: f32) -> f32 {
    let b = x.to_bits();
    f32::from_bits(if x == 0.0 {
        1
    } else if x > 0.0 {
        b + 1
    } else {
        b - 1
    })
}
fn below(x: f32) -> f32 {
    -above(-x)
}

pub fn generate() -> Cohort {
    let mut b = Builder {
        seen: BTreeSet::new(),
        coordinates: Vec::new(),
    };
    b.add([0.5; 3]); // Index zero is the sole fixed center anchor in every case.
    for pivot in [-0.5_f32, 0.0, 0.5, 1.0, 1.5] {
        b.sweep(pivot);
        let mut lo = pivot;
        let mut hi = pivot;
        for _ in 0..4 {
            lo = below(lo);
            hi = above(hi);
            b.sweep(lo);
            b.sweep(hi);
        }
        for delta in [1.0 / 65536.0, 1.0 / 4096.0, 1.0 / 512.0] {
            b.sweep(pivot - delta);
            b.sweep(pivot + delta);
        }
    }
    b.sweep(-0.0);
    for n in [1, 3, 127, 129, 255, 257, 383, 385, 511, 513] {
        b.sweep(n as f32 / 512.0);
    }
    for x in [0.0, 0.25, 0.75, 1.0] {
        for y in [0.0, 0.25, 0.75, 1.0] {
            for z in [0.0, 0.25, 0.75, 1.0] {
                b.add([x, y, z]);
            }
        }
    }
    for magnitude in [2.0_f32, 16.0, 4096.0, 16777216.0, f32::MAX] {
        b.sweep(magnitude);
        b.sweep(-magnitude);
    }
    // Distinguish normalized-coordinate rounding from subnormal flushing.
    // These are input probes, not assumed filtering expectations.
    for bits in [
        0x0000_0008, 0x0000_0100, 0x0001_0000, 0x0040_0000,
        0x007f_ffff, 0x0080_0000, 0x0080_0001, 0x0d80_0000,
        0x2180_0000, 0x2f80_0000, 0x3280_0000, 0x3300_0000,
        0x3380_0000, 0x3400_0000, 0x3480_0000, 0x3580_0000,
    ] {
        let magnitude = f32::from_bits(bits);
        b.sweep(magnitude);
        b.sweep(-magnitude);
    }
    let core_count = b.coordinates.len();

    // Exact k/512 coordinates plus both adjacent binary32 numbers at odd k.
    // These are observations, not an implementation of a filtering oracle.
    for n in -256..=768 {
        let x = n as f32 / 512.0;
        b.sweep(x);
        if n & 1 != 0 {
            b.sweep(below(x));
            b.sweep(above(x));
        }
    }
    for x in GRID_WEIGHTS {
        for y in GRID_WEIGHTS {
            for z in GRID_WEIGHTS {
                b.add([x as f32 / 512.0, y as f32 / 512.0, z as f32 / 512.0]);
            }
        }
    }
    Cohort {
        coordinates: b.coordinates,
        core_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_unique_and_deterministic() {
        let a = generate();
        let b = generate();
        assert_eq!(a.coordinates, b.coordinates);
        assert_eq!(a.core_count, b.core_count);
        assert_eq!(a.coordinates[0], CENTER);
        assert_eq!(
            a.coordinates.iter().copied().collect::<BTreeSet<_>>().len(),
            a.coordinates.len()
        );
        assert!(a
            .coordinates
            .iter()
            .flatten()
            .all(|v| f32::from_bits(*v).is_finite()));
        assert!(a.core_count > 300 && a.core_count < 600);
        assert!(a.coordinates.len() > 11000 && a.coordinates.len() < 13000);
    }
    #[test]
    fn tie_neighbors_r_axis_and_far_values_are_present() {
        let c = generate();
        for n in [-255, 1, 127, 257, 511, 767] {
            let x = n as f32 / 512.0;
            for v in [below(x), x, above(x)] {
                assert!(c.coordinates.contains(&[
                    0.5_f32.to_bits(),
                    0.5_f32.to_bits(),
                    v.to_bits()
                ]));
                assert!(c.coordinates.contains(&[v.to_bits(); 3]));
            }
        }
        assert!(c.coordinates.contains(&[
            0.5_f32.to_bits(),
            0.5_f32.to_bits(),
            f32::MAX.to_bits()
        ]));
        assert!(c.coordinates.contains(&[(-0.0_f32).to_bits(); 3]));
    }
    #[test]
    fn float_neighbors_cross_zero_correctly() {
        assert_eq!(above(-0.0).to_bits(), 1);
        assert_eq!(below(0.0).to_bits(), 0x8000_0001);
        assert_eq!(above(-0.5).to_bits(), (-0.5_f32).to_bits() - 1);
    }
}
