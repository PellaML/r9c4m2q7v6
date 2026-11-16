//! Small, deterministic input geometry and diagnostics, never a coverage model.
use crate::cases::{self, Case, BASE, INDEXED, KEEP, REPEAT};
use crate::ffi::{Input, SLAB};

pub const CASE_COUNT: usize = cases::CASE_COUNT;
pub const MEASUREMENT_COUNT: usize = 2 * CASE_COUNT;
pub const BATCH_SECONDS: u64 = 240;
pub const RAW_BYTES: usize = CASE_COUNT * 8 * SLAB;

#[derive(Clone, Debug)]
pub struct Pair {
    pub lhs: usize,
    pub rhs: usize,
    pub kind: &'static str,
    // None means observation, never an assertion of equal or unequal masks.
    pub expected_equal: Option<bool>,
}
pub struct Suite {
    pub cases: Vec<Case>,
    pub pairs: Vec<Pair>,
}

pub fn generate() -> Suite {
    let mut pairs = Vec::new();
    for (rhs, kind, equal) in [
        (REPEAT, "exact_repeat", true),
        (INDEXED, "direct_indexed_equivalence", true),
        (KEEP, "wrong_keep_equality_rejected", false),
    ] {
        pairs.push(Pair {
            lhs: BASE,
            rhs,
            kind,
            expected_equal: Some(equal),
        });
    }
    for lhs in (6..CASE_COUNT).step_by(2) {
        pairs.push(Pair {
            lhs,
            rhs: lhs + 1,
            kind: "endpoint_reversal_observation",
            expected_equal: None,
        });
    }
    Suite {
        cases: cases::generate(),
        pairs,
    }
}

pub fn ideal_window(clip: [f32; 2], viewport: [f32; 4]) -> [f64; 2] {
    [
        f64::from(viewport[0]) + (f64::from(clip[0]) + 1.0) * f64::from(viewport[2]) * 0.5,
        f64::from(viewport[1]) + (1.0 - f64::from(clip[1])) * f64::from(viewport[3]) * 0.5,
    ]
}
pub fn consumed_indices(p: &Input) -> Vec<usize> {
    (0..p.draw_count as usize)
        .map(|i| {
            if p.indexed != 0 {
                p.indices[p.index_first as usize + i] as usize
            } else {
                p.vertex_start as usize + i
            }
        })
        .collect()
}
pub fn reference_clip_bits(p: &Input) -> Vec<[u32; 2]> {
    assert_eq!(p.position_mode, 1);
    consumed_indices(p)
        .into_iter()
        .map(|vid| p.vertices[vid].map(f32::to_bits))
        .collect()
}

#[derive(Clone, Debug)]
pub struct Margin {
    pub major_axis: usize,
    pub major_sample_twice: i32,
    pub nearest_minor_integer: i64,
    pub signed_numerator: i64,
    pub denominator: i64,
    pub samples_tested: usize,
    pub exact_tie_samples: usize,
}

// Exact rational geometry certificate, not a rasterizer: at major-coordinate
// k+1/2, compare the segment's minor ordinate with the nearest integer. These
// locations are diamond-tip landmarks, but no Metal decision rule is assumed.
// Inspect only samples >=8 major-axis pixels away from BOTH endpoints.
pub fn interior_margin(c: &Case) -> Option<Margin> {
    if c.window_grid_256.len() != 2 {
        return None;
    }
    let [a, b] = [c.window_grid_256[0], c.window_grid_256[1]];
    let d = [b[0] - a[0], b[1] - a[1]];
    let major = usize::from(d[1].abs() > d[0].abs());
    let minor = 1 - major;
    let mut best: Option<Margin> = None;
    let mut samples = 0;
    let mut ties = 0;
    for k in 0..crate::ffi::SIDE as i32 {
        let x = 256 * k + 128;
        if x < a[major].min(b[major]) + 8 * 256 || x > a[major].max(b[major]) - 8 * 256 {
            continue;
        }
        samples += 1;
        let mut n = i64::from(a[minor]) * i64::from(d[major])
            + i64::from(d[minor]) * i64::from(x - a[major]);
        let mut den = 256 * i64::from(d[major]);
        if den < 0 {
            den = -den;
            n = -n;
        }
        let nearest = (n + den / 2).div_euclid(den);
        let signed = n - nearest * den;
        if signed == 0 {
            ties += 1;
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|old| signed.abs() * old.denominator < old.signed_numerator.abs() * den)
        {
            best = Some(Margin {
                major_axis: major,
                major_sample_twice: 2 * k + 1,
                nearest_minor_integer: nearest,
                signed_numerator: signed,
                denominator: den,
                samples_tested: 0,
                exact_tie_samples: 0,
            });
        }
    }
    best.map(|mut m| {
        m.samples_tested = samples;
        m.exact_tie_samples = ties;
        m
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimensions_and_raw_budget_are_exact() {
        assert_eq!(crate::ffi::SIDE, 256);
        assert_eq!(crate::ffi::ROW, 2048);
        assert_eq!(SLAB, 524800);
        assert_eq!(8 * SLAB, 4198400);
        assert_eq!(MEASUREMENT_COUNT, 32);
        assert_eq!(RAW_BYTES, 67174400);
        const { assert!(RAW_BYTES < 75 * 1024 * 1024) };
    }
    #[test]
    fn rational_interior_certificates_are_tight_and_not_endpoint_events() {
        let s = generate();
        for id in [6, 8, 10, 12, 14] {
            let m = interior_margin(&s.cases[id]).unwrap();
            let reverse = interior_margin(&s.cases[id + 1]).unwrap();
            assert!(m.samples_tested >= 40);
            assert_eq!(m.exact_tie_samples, 0);
            assert_eq!(
                (m.signed_numerator, m.denominator),
                (reverse.signed_numerator, reverse.denominator)
            );
            assert!(m.signed_numerator.abs() * 256 <= m.denominator);
            assert!(m.signed_numerator.abs() * 65536 >= m.denominator);
        }
        for id in [12, 14] {
            let m = interior_margin(&s.cases[id]).unwrap();
            assert_eq!(m.signed_numerator.abs() * 65536, m.denominator);
        }
    }
    #[test]
    fn only_controls_have_gating_pair_predictions() {
        let s = generate();
        assert_eq!(s.pairs.len(), 8);
        assert_eq!(
            s.pairs
                .iter()
                .filter(|p| p.expected_equal.is_some())
                .count(),
            3
        );
        assert!(s
            .pairs
            .iter()
            .filter(|p| p.kind == "endpoint_reversal_observation")
            .all(|p| p.expected_equal.is_none()));
    }
}
