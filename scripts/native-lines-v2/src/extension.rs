//! Deterministic synthetic inputs and comparison recipes, never a coverage model.
use crate::cases::{self, Case};
use crate::ffi::Input;

pub const CASE_COUNT: usize = 962;
pub const MEASUREMENT_COUNT: usize = 2 * CASE_COUNT;
pub const GENERATOR_SEED: u32 = 0x8d63_5fa7;
pub const BATCH_SECONDS: u64 = 240;

#[derive(Clone, Debug)]
pub struct Pair {
    pub lhs: usize,
    pub rhs: usize,
    pub kind: &'static str,
    // None is an observation, NOT an assertion that masks must match or differ.
    pub expected_equal: Option<bool>,
}
pub struct Suite {
    pub cases: Vec<Case>,
    pub pairs: Vec<Pair>,
}

// Defined here rather than relying on a recently stabilized standard-library API.
// The generator uses only finite values; these also preserve the signed-zero edge.
pub fn next_up(x: f32) -> f32 {
    assert!(x.is_finite());
    if x == 0.0 {
        return f32::from_bits(1);
    }
    f32::from_bits(if x > 0.0 {
        x.to_bits() + 1
    } else {
        x.to_bits() - 1
    })
}
pub fn next_down(x: f32) -> f32 {
    -next_up(-x)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionReference {
    pub after_offset: [f32; 2],
    pub relative: [f32; 2],
    pub divided: [f32; 2],
    pub scaled: [f32; 2],
    pub clip: [f32; 2],
    pub ideal_window: [f64; 2],
}

// CPU staged binary32 reference ONLY. A compiler/GPU may contract or otherwise
// evaluate the shader differently. These are not native-readback vertex bits.
pub fn project_reference(p: [f32; 2], offset: [f32; 2], viewport: [f32; 4]) -> ProjectionReference {
    let after_offset = [p[0] + offset[0], p[1] + offset[1]];
    let relative = [after_offset[0] - viewport[0], after_offset[1] - viewport[1]];
    let divided = [relative[0] / viewport[2], relative[1] / viewport[3]];
    let scaled = [divided[0] * 2.0, divided[1] * -2.0];
    let clip = [scaled[0] - 1.0, scaled[1] + 1.0];
    ProjectionReference {
        after_offset,
        relative,
        divided,
        scaled,
        clip,
        ideal_window: ideal_window(clip, viewport),
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
    (0..p.instances as usize)
        .flat_map(|iid| {
            consumed_indices(p).into_iter().map(move |vid| {
                let xy = if p.position_mode == 1 {
                    p.vertices[vid]
                } else {
                    project_reference(p.vertices[vid], p.offsets[iid], p.viewport).clip
                };
                xy.map(f32::to_bits)
            })
        })
        .collect()
}

impl Suite {
    fn relation(
        &mut self,
        lhs: usize,
        rhs: usize,
        kind: &'static str,
        expected_equal: Option<bool>,
    ) {
        self.pairs.push(Pair {
            lhs,
            rhs,
            kind,
            expected_equal,
        });
    }
    fn add(&mut self, mut c: Case, family: &'static str, recipe: String) -> usize {
        let id = self.cases.len();
        c.input.id = id as u32;
        c.name = format!("{family}_{id:04}");
        c.family = family;
        c.recipe = recipe;
        c.role = "generated_observation";
        self.cases.push(c);
        id
    }
    fn pair(
        &mut self,
        points: [[f32; 2]; 2],
        viewport: [f32; 4],
        position_mode: u32,
        family: &'static str,
        recipe: String,
    ) -> [usize; 2] {
        let mut ids = [0; 2];
        for (direction, id) in ids.iter_mut().enumerate() {
            let p = if direction == 0 {
                points
            } else {
                [points[1], points[0]]
            };
            let mut c = cases::direct("generated", &p);
            c.input.viewport = viewport;
            c.input.position_mode = position_mode;
            *id = self.add(c, family, format!("{recipe}; direction={direction}"));
        }
        self.relation(ids[0], ids[1], "endpoint_reversal_observation", None);
        ids
    }
    fn clip_companion(
        &mut self,
        source: usize,
        family: &'static str,
        expected: Option<bool>,
    ) -> usize {
        let mut c = self.cases[source].clone();
        assert_eq!(c.input.instances, 1);
        assert_eq!(c.input.offsets, [[0.0; 2]; 3]);
        assert_eq!(c.input.position_mode, 0);
        for p in &mut c.input.vertices {
            *p = project_reference(*p, [0.0; 2], c.input.viewport).clip;
        }
        c.input.position_mode = 1;
        let id = self.add(c, family, format!("direct clip from CPU staged binary32 reference of case {source}; not GPU vertex readback"));
        self.relation(
            source,
            id,
            if expected.is_some() {
                "exact_dyadic_path_control"
            } else {
                "window_vs_cpu_reference_direct_clip_observation"
            },
            expected,
        );
        id
    }
}

fn transform(mut points: [[f32; 2]; 2], symmetry: usize) -> [[f32; 2]; 2] {
    for p in &mut points {
        if symmetry & 1 != 0 {
            p.swap(0, 1);
        }
        if symmetry & 2 != 0 {
            p[0] = 32.0 - p[0];
        }
        if symmetry & 4 != 0 {
            p[1] = 32.0 - p[1];
        }
    }
    points
}
fn transpose(mut points: [[f32; 2]; 2], yes: bool) -> [[f32; 2]; 2] {
    if yes {
        for p in &mut points {
            p.swap(0, 1);
        }
    }
    points
}
fn random_word(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}
fn unit_fraction(state: &mut u32) -> f32 {
    f32::from_bits(0x3f80_0000 | (random_word(state) & 0x007f_ffff)) - 1.0
}

pub fn generate() -> Suite {
    let mut s = Suite {
        cases: cases::generate(),
        pairs: Vec::new(),
    };
    let full = [0.0, 0.0, 32.0, 32.0];

    // Four independently varied endpoint components, not a common translation.
    for denominator in [8u32, 16, 256, 4096] {
        for seed in 0..2u32 {
            let nums = [
                37 + seed * 31,
                503 + seed * 73,
                1193 + seed * 151,
                3001 + seed * 293,
            ];
            let f = nums.map(|n| (n % denominator) as f32 / denominator as f32);
            let points = [
                [4.0 + f[0], 9.0 + f[1]],
                [27.0 + f[2], 10.0 + seed as f32 * 3.0 + f[3]],
            ];
            for symmetry in 0..8 {
                s.pair(transform(points, symmetry), full, 0, "independent_fraction_phases",
                    format!("denominator={denominator}; seed={seed}; symmetry={symmetry}; four independent phase numerators"));
            }
        }
    }
    let mut axis_pairs = Vec::new();
    for anchor in [10.0f32, 10.5] {
        for k in -3..=3 {
            let center = anchor + k as f32 / 512.0;
            let mut neighbors = Vec::new();
            for (neighbor, y) in [next_down(center), center, next_up(center)]
                .into_iter()
                .enumerate()
            {
                for trans in 0..2 {
                    let ids = s.pair(transpose([[5.5, y], [26.5, y]], trans == 1), full, 0,
                        "axis_threshold_neighbors", format!("anchor={anchor}; k_over_512={k}; neighbor={neighbor}; transpose={trans}"));
                    axis_pairs.push(ids);
                    neighbors.push(ids);
                }
            }
            for trans in 0..2 {
                for (direction, &center_id) in neighbors[2 + trans].iter().enumerate() {
                    for neighbor in [0, 2] {
                        s.relation(
                            center_id,
                            neighbors[neighbor * 2 + trans][direction],
                            "one_ulp_axis_neighbor_observation",
                            None,
                        );
                    }
                }
            }
        }
    }
    let mut near_pairs = Vec::new();
    for phase in [0.0f32, 1.0 / 1024.0, 1.0 / 512.0, 1.0 / 256.0] {
        let x = 11.0 + phase;
        for (minor, end_x) in [
            x - 1.0 / 64.0,
            x - 1.0 / 256.0,
            x + 1.0 / 256.0,
            x + 1.0 / 64.0,
            next_up(x),
            next_down(x),
        ]
        .into_iter()
        .enumerate()
        {
            assert_ne!(x.to_bits(), end_x.to_bits());
            for trans in 0..2 {
                near_pairs.push(s.pair(transpose([[x, 5.25], [end_x, 26.75]], trans == 1), full, 0,
                    "near_axis_tiny_minor", format!("phase={phase}; minor_variant={minor}; transpose={trans}; input minor delta is nonzero")));
            }
        }
    }
    let mut short_pairs = Vec::new();
    for (anchor_id, a) in [[14.0f32, 18.0], [14.5, 18.5], [14.25, 18.75]]
        .into_iter()
        .enumerate()
    {
        for length in 0..8 {
            for axis in 0..2 {
                let mut b = a;
                b[axis] = match length {
                    0 => a[axis],
                    1 => next_up(a[axis]),
                    _ => {
                        a[axis]
                            + [1.0 / 4096.0, 1.0 / 256.0, 1.0 / 16.0, 1.0 / 8.0, 0.5, 0.75]
                                [length - 2]
                    }
                };
                short_pairs.push(s.pair([a, b], full, 0, "short_axis_and_degenerate",
                    format!("anchor={anchor_id}; length_variant={length}; axis={axis}; length 0 is exact degeneracy, 1 is adjacent f32")));
            }
        }
        for d in [1.0f32 / 4096.0, 1.0 / 256.0, 0.5, 0.75] {
            s.pair(
                [a, [a[0] + d, a[1] + d * 0.5]],
                full,
                0,
                "short_oblique",
                format!("anchor={anchor_id}; dx={d}; dy=dx/2"),
            );
        }
    }

    // Diamond landmarks are sample locations only; no ownership rule is asserted.
    for (seed, points) in [
        [[5.25f32, 11.25], [25.75, 16.5]],
        [[5.5, 12.0], [25.0, 22.5]],
    ]
    .into_iter()
    .enumerate()
    {
        let mut bases = [[0; 2]; 2];
        for variant in 0..17 {
            let mut p = points;
            if variant > 0 {
                let coord = (variant - 1) / 4;
                let kind = (variant - 1) % 4;
                let v = &mut p[coord / 2][coord % 2];
                *v = match kind {
                    0 => next_down(*v),
                    1 => next_up(*v),
                    2 => *v - 1.0 / 4096.0,
                    _ => *v + 1.0 / 4096.0,
                };
            }
            for (trans, base) in bases.iter_mut().enumerate() {
                let ids = s.pair(transpose(p, trans == 1), full, 0, "independent_endpoint_neighbors",
                    format!("landmark_seed={seed}; variant={variant}; transpose={trans}; only one of four coordinates changes"));
                if variant == 0 {
                    *base = ids;
                } else {
                    for direction in 0..2 {
                        s.relation(
                            base[direction],
                            ids[direction],
                            "endpoint_phase_neighbor_observation",
                            None,
                        );
                    }
                }
            }
        }
    }
    let mut state = GENERATOR_SEED;
    let mut arbitrary_pairs = Vec::new();
    for seed in 0..16 {
        let a = [
            4.0 + unit_fraction(&mut state) * 8.0,
            6.0 + unit_fraction(&mut state) * 16.0,
        ];
        let b = [
            20.0 + unit_fraction(&mut state) * 8.0,
            6.0 + unit_fraction(&mut state) * 16.0,
        ];
        arbitrary_pairs.push(s.pair(
            [a, b],
            full,
            0,
            "binary32_mantissa_window",
            format!("xorshift32 seed index={seed}; four independent mantissas; no external data"),
        ));
    }

    // Six forward/reverse pairs, selected across distinct precision families.
    let selected = [
        axis_pairs[0],
        axis_pairs[42],
        near_pairs[8],
        near_pairs[10],
        short_pairs[4],
        arbitrary_pairs[0],
    ];
    for source in selected.into_iter().flatten() {
        s.clip_companion(source, "projection_direct_reference", None);
        for viewport in [
            [2.0, 4.0, 28.0, 24.0],
            [0.0, 0.0, 384.0, 192.0],
            [0.0, 0.0, 512.0, 512.0],
        ] {
            let mut c = s.cases[source].clone();
            c.input.viewport = viewport;
            let window = s.add(c, "projection_window_repeat", format!("same nominal window inputs as case {source}; viewport={viewport:?}; 32x32 target/scissor unchanged"));
            s.relation(source, window, "viewport_projection_observation", None);
            s.clip_companion(window, "projection_direct_reference", None);
        }
    }

    for anchor in [-0.5f32, -0.25, 0.25, 0.5] {
        for (neighbor, y) in [next_down(anchor), anchor, next_up(anchor)]
            .into_iter()
            .enumerate()
        {
            for trans in 0..2 {
                s.pair(transpose([[-0.6875, y], [0.6875, anchor]], trans == 1), full, 1,
                    "direct_clip_ulp_neighbors", format!("clip anchor={anchor}; first-endpoint neighbor={neighbor}; transpose={trans}; no window transform"));
            }
        }
    }
    for seed in 0..16 {
        let a = [
            -f32::from_bits(0x3f00_0000 | (random_word(&mut state) & 0x007f_ffff)),
            f32::from_bits(0x3e00_0000 | (random_word(&mut state) & 0x807f_ffff)),
        ];
        let b = [
            f32::from_bits(0x3f00_0000 | (random_word(&mut state) & 0x007f_ffff)),
            f32::from_bits(0x3e00_0000 | (random_word(&mut state) & 0x807f_ffff)),
        ];
        s.pair([a, b], full, 1, "binary32_mantissa_direct_clip",
            format!("xorshift32 direct bit construction index={seed}; finite normal clip values, all inside [-1,1]"));
    }

    let original = [[-4.0f32, 8.0], [36.0, 24.0]];
    // Analytic intersections of the already-public baseline line with x=8,24.
    // Rounded explicit INPUTS, not predicted rasterized pixels.
    let preclipped = [[8.0, 64.0 / 5.0], [24.0, 96.0 / 5.0]];
    for trans in 0..2 {
        for viewport in [full, [8.0, 8.0, 16.0, 16.0]] {
            let mut ids = [[0; 2]; 2];
            for (shape, p) in [original, preclipped].into_iter().enumerate() {
                for (direction, id) in ids[shape].iter_mut().enumerate() {
                    let p = transpose(if direction == 0 { p } else { [p[1], p[0]] }, trans == 1);
                    if trans == 0 && viewport != full && shape == 0 && direction == 0 {
                        *id = 54; // Reuse, never replace or alter baseline 54.
                    } else {
                        let mut c = cases::direct("clip", &p);
                        c.input.viewport = viewport;
                        *id = s.add(c, "public_inset_clip_repeats",
                            format!("public baseline geometry; shape={shape} (0 original, 1 explicit analytic preclip); transpose={trans}; direction={direction}; no equality assumed"));
                    }
                }
                s.relation(
                    ids[shape][0],
                    ids[shape][1],
                    "endpoint_reversal_observation",
                    None,
                );
            }
            for (&original_id, &preclipped_id) in ids[0].iter().zip(&ids[1]) {
                s.relation(
                    original_id,
                    preclipped_id,
                    "original_vs_explicit_preclip_observation",
                    None,
                );
                s.clip_companion(preclipped_id, "preclip_direct_reference", None);
            }
        }
    }
    for (seed, p) in [[[-6.0f32, 7.0], [38.0, 25.0]], [[-6.0, 24.0], [38.0, 8.0]]]
        .into_iter()
        .enumerate()
    {
        // Construct two known x-plane intersections in f64 then explicitly round
        // to binary32 inputs. This is not a general clipping or coverage kernel.
        let y = |x: f64| {
            (f64::from(p[0][1])
                + (x - f64::from(p[0][0])) / f64::from(p[1][0] - p[0][0])
                    * f64::from(p[1][1] - p[0][1])) as f32
        };
        let clipped = [[2.0, y(2.0)], [30.0, y(30.0)]];
        for trans in 0..2 {
            let viewport = if trans == 0 {
                [2.0, 4.0, 28.0, 24.0]
            } else {
                [4.0, 2.0, 24.0, 28.0]
            };
            let a = s.pair(
                transpose(p, trans == 1),
                viewport,
                0,
                "oblique_nonpower_clip",
                format!("seed={seed}; transpose={trans}; original endpoints across x planes"),
            );
            let b = s.pair(
                transpose(clipped, trans == 1),
                viewport,
                0,
                "oblique_nonpower_clip",
                format!(
                    "seed={seed}; transpose={trans}; explicit rounded analytic intersection inputs"
                ),
            );
            for direction in 0..2 {
                s.relation(
                    a[direction],
                    b[direction],
                    "original_vs_explicit_preclip_observation",
                    None,
                );
            }
        }
    }
    for source in [
        axis_pairs[1][0],
        near_pairs[8][1],
        short_pairs[5][0],
        arbitrary_pairs[1][1],
    ] {
        let indexed = cases::indexed(&s.cases[source], "indexed");
        let id = s.add(
            indexed,
            "indexed_precision_control",
            format!("shuffled UInt16 equivalent of case {source}"),
        );
        s.relation(source, id, "direct_indexed_control", Some(true));
    }
    for source in [
        near_pairs[8][0],
        short_pairs[0][0],
        arbitrary_pairs[2][0],
        54,
    ] {
        let id = s.add(
            s.cases[source].clone(),
            "repeatability_control",
            format!("same exact input as case {source}, separate command buffer"),
        );
        s.relation(source, id, "exact_repeat_control", Some(true));
    }
    let mut clip_controls = Vec::new();
    for source in [32, 34, 36, 38] {
        clip_controls.push(s.clip_companion(source, "direct_clip_exact_control", Some(true)));
    }
    let mut keep = s.cases[clip_controls[0]].clone();
    keep.input.keep = 1;
    keep.role = "negative_control";
    let keep_id = s.add(
        keep,
        "direct_clip_keep_control",
        "same positive direct-clip geometry; stencil Keep must stay empty".into(),
    );
    s.relation(
        clip_controls[0],
        keep_id,
        "direct_clip_keep_wrong_equality_rejected",
        Some(false),
    );
    assert_eq!(
        s.cases.len(),
        CASE_COUNT,
        "fixed public generator count changed"
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjacent_binary32_steps_are_exact() {
        for x in [5.0, 10.5, -0.5, -0.25, 0.25, 512.0] {
            assert!(next_down(x) < x && next_up(x) > x);
            assert_eq!(next_down(next_up(x)).to_bits(), x.to_bits());
        }
        assert_eq!(next_up(-0.0).to_bits(), 1);
        assert_eq!(next_down(0.0).to_bits(), 0x8000_0001);
    }
    #[test]
    fn preserves_every_baseline_input_and_order() {
        let old = cases::generate();
        let s = generate();
        assert_eq!(old.len(), 66);
        for (a, b) in old.iter().zip(&s.cases) {
            assert_eq!(a.input, b.input);
            assert_eq!(a.name, b.name);
        }
    }
    #[test]
    fn generated_inputs_are_deterministic_finite_bounded_and_tagged() {
        let a = generate();
        let b = generate();
        assert_eq!(a.cases.len(), CASE_COUNT);
        assert!(a.cases.len() < 1024);
        for (i, (a, b)) in a.cases.iter().zip(&b.cases).enumerate() {
            assert_eq!(a.input, b.input);
            assert_eq!(a.recipe, b.recipe);
            let p = &a.input;
            assert_eq!(p.id as usize, i);
            assert!(p
                .vertices
                .iter()
                .flatten()
                .all(|x| x.is_finite() && x.abs() <= 128.0));
            assert!(
                p.viewport[2] > 0.0
                    && p.viewport[2] <= 512.0
                    && p.viewport[3] > 0.0
                    && p.viewport[3] <= 512.0
            );
            assert!(p.scissor[0] + p.scissor[2] <= 32 && p.scissor[1] + p.scissor[3] <= 32);
            assert!(consumed_indices(p).iter().all(|&v| v < 8));
            if p.position_mode == 1 {
                assert_eq!(p.instances, 1);
                assert_eq!(p.offsets, [[0.0; 2]; 3]);
            }
        }
        assert!(a.cases.len() * 8 * crate::ffi::SLAB < 75 * 1024 * 1024);
    }
    #[test]
    fn comparison_policy_does_not_guess_reversal_quantization_or_clipping() {
        let s = generate();
        for p in &s.pairs {
            assert!(p.lhs < CASE_COUNT && p.rhs < CASE_COUNT);
            if p.expected_equal.is_some() {
                assert!(matches!(
                    p.kind,
                    "exact_dyadic_path_control"
                        | "direct_indexed_control"
                        | "exact_repeat_control"
                        | "direct_clip_keep_wrong_equality_rejected"
                ));
            }
        }
        assert!(s
            .pairs
            .iter()
            .any(|p| p.kind == "original_vs_explicit_preclip_observation" && p.lhs == 54));
    }
    #[test]
    fn tiny_nonzero_input_deltas_and_projection_collapse_are_distinct() {
        let s = generate();
        for c in s
            .cases
            .iter()
            .filter(|c| c.family == "near_axis_tiny_minor")
        {
            let p = &c.input;
            let a = p.vertices[1];
            let b = p.vertices[2];
            assert_ne!(a[0].to_bits(), b[0].to_bits());
            assert_ne!(a[1].to_bits(), b[1].to_bits());
        }
        let a = project_reference([3.0, 5.0], [0.0; 2], [0.0, 0.0, 32.0, 32.0]);
        let b = project_reference([next_up(3.0), 5.0], [0.0; 2], [0.0, 0.0, 32.0, 32.0]);
        assert_ne!(a.after_offset[0].to_bits(), b.after_offset[0].to_bits());
        assert_eq!(
            a.clip[0].to_bits(),
            b.clip[0].to_bits(),
            "CPU reference collapse, NOT a native coverage claim"
        );
    }
    #[test]
    fn exact_direct_clip_controls_have_exact_dyadic_positions() {
        let s = generate();
        for pair in s
            .pairs
            .iter()
            .filter(|p| p.kind == "exact_dyadic_path_control")
        {
            let a = &s.cases[pair.lhs].input;
            let b = &s.cases[pair.rhs].input;
            assert_eq!(reference_clip_bits(a), reference_clip_bits(b));
            for v in consumed_indices(a) {
                assert_eq!(
                    project_reference(a.vertices[v], [0.0; 2], a.viewport).ideal_window,
                    a.vertices[v].map(f64::from)
                );
            }
        }
    }
}
