//! Deterministic synthetic inputs and comparison recipes, never a coverage model.
use crate::cases::{self, Case};
use crate::ffi::Input;

pub const CASE_COUNT: usize = 485;
pub const CASE_LIMIT: usize = 512;
pub const RAW_LIMIT_BYTES: usize = 36 * 1024 * 1024;
pub const MEASUREMENT_COUNT: usize = 2 * CASE_COUNT;
pub const BATCH_SECONDS: u64 = 120;

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
// Fixed public seeds only: no RNG, input files, captured data, or raster model.
const FULL: [f32; 4] = [0.0, 0.0, 32.0, 32.0];
const PHASES: [f32; 5] = [0.0, -0.25, 0.25, -0.5, 0.5];
const NEIGHBORS: [f32; 7] = [
    0.0,
    -1.0 / 256.0,
    1.0 / 256.0,
    -1.0 / 512.0,
    1.0 / 512.0,
    -1.0 / 4096.0,
    1.0 / 4096.0,
];

fn diagonal(fraction: f32, phase: f32) -> [[f32; 2]; 2] {
    [
        [4.0 + fraction, 4.0 + fraction + phase],
        [27.0 + fraction, 27.0 + fraction + phase],
    ]
}

// Return [window forward/reverse, direct-clip forward/reverse]. All new path
// comparisons are observations, even when their dyadic input arithmetic is exact.
fn window_and_clip(
    s: &mut Suite,
    points: [[f32; 2]; 2],
    window_family: &'static str,
    clip_family: &'static str,
    recipe: String,
) -> [[usize; 2]; 2] {
    let window = s.pair(points, FULL, 0, window_family, recipe);
    let clip = window.map(|id| s.clip_companion(id, clip_family, None));
    s.relation(clip[0], clip[1], "endpoint_reversal_observation", None);
    [window, clip]
}

fn compare_groups(s: &mut Suite, a: [[usize; 2]; 2], b: [[usize; 2]; 2], kind: &'static str) {
    for path in 0..2 {
        for direction in 0..2 {
            s.relation(a[path][direction], b[path][direction], kind, None);
        }
    }
}

pub fn generate() -> Suite {
    let mut s = Suite {
        cases: cases::generate(),
        pairs: Vec::new(),
    };
    let mut centers = Vec::new();
    // Full local phase sweep at the .5 starting fraction. Exact 23-by-23
    // nominal deltas, including sample-diamond corner landmarks at +/- .5.
    // A landmark is a geometric input location, NOT an expected covered sample.
    for phase in PHASES {
        for (neighbor, delta) in NEIGHBORS.into_iter().enumerate() {
            let ids = window_and_clip(&mut s, diagonal(0.5, phase + delta),
                "diagonal_phase_window", "diagonal_phase_direct_clip",
                format!("fixed seed (4.5,4.5+phase) to (27.5,27.5+phase); phase_anchor={phase}; phase_delta={delta}; symmetry=0"));
            if neighbor == 0 {
                centers.push(ids);
            } else {
                compare_groups(
                    &mut s,
                    *centers.last().unwrap(),
                    ids,
                    "diagonal_phase_neighbor_observation",
                );
            }
        }
    }
    // Complete D4 at each anchor: swap axes, reflect x, reflect y, and their
    // compositions about the 32x32 viewport. Identity is already in the sweep.
    for (phase_index, phase) in PHASES.into_iter().enumerate() {
        for symmetry in 1..8 {
            let ids = window_and_clip(&mut s, transform(diagonal(0.5, phase), symmetry),
                "diagonal_symmetry_window", "diagonal_symmetry_direct_clip",
                format!("starting_fraction=0.5; phase_anchor={phase}; D4_bits={symmetry}; bit0=swap_xy,bit1=reflect_x,bit2=reflect_y"));
            compare_groups(
                &mut s,
                centers[phase_index],
                ids,
                "diagonal_symmetry_observation",
            );
        }
    }
    // Starting-major fractions .125, .25, .5, .875; .5 is reused above.
    for fraction in [0.125, 0.25, 0.875] {
        for phase_index in [0, 3, 4] {
            let phase = PHASES[phase_index];
            let ids = window_and_clip(&mut s, diagonal(fraction, phase),
                "diagonal_start_fraction_window", "diagonal_start_fraction_direct_clip",
                format!("starting_fraction={fraction}; phase_anchor={phase}; both endpoint components translated equally from half-start seed"));
            compare_groups(
                &mut s,
                centers[phase_index],
                ids,
                "starting_major_fraction_observation",
            );
        }
    }
    // Translate both y values by one ULP OF THE FAR WINDOW ENDPOINT. The near
    // endpoint moves by the same exact displacement (not necessarily one ULP).
    // This preserves exact 45 degrees; stepping the phase scalar would round away.
    for phase_index in [3, 4] {
        let phase = PHASES[phase_index];
        for up in [false, true] {
            let mut points = diagonal(0.5, phase);
            let far = points[1][1];
            let delta = if up { next_up(far) } else { next_down(far) } - far;
            for p in &mut points {
                p[1] += delta;
            }
            let ids = window_and_clip(&mut s, points,
                "diagonal_common_ulp_window", "diagonal_common_ulp_direct_clip",
                format!("phase_anchor={phase}; far_window_y_adjacent_up={up}; common_y_delta={delta}; exact45 preserved; near endpoint uses same delta"));
            compare_groups(
                &mut s,
                centers[phase_index],
                ids,
                "common_translation_ulp_observation",
            );
        }
    }
    // Bracket |dx|=|dy| by changing ONE endpoint component by its adjacent f32.
    // These are intentionally NEAR-45, not falsely labeled exact45. First-endpoint
    // changes may collapse in the CPU projection; direct companions expose that
    // reference separately without claiming GPU projection or snapping behavior.
    for endpoint in 0..2 {
        for axis in 0..2 {
            for up in [false, true] {
                let mut points = diagonal(0.5, 0.5);
                let v = &mut points[endpoint][axis];
                *v = if up { next_up(*v) } else { next_down(*v) };
                let ids = window_and_clip(&mut s, points,
                    "diagonal_major_neighbor_window", "diagonal_major_neighbor_direct_clip",
                    format!("phase_anchor=0.5; endpoint={endpoint}; axis={axis}; adjacent_f32_up={up}; one component only; near45 major-axis bracket, not exact45"));
                compare_groups(
                    &mut s,
                    centers[4],
                    ids,
                    "near_45_major_axis_neighbor_observation",
                );
            }
        }
    }
    let short_anchor = [3.0f32, 3.5];
    let lengths = [
        0.0,
        next_up(3.0) - 3.0,
        1.0 / 4096.0,
        1.0 / 512.0,
        1.0 / 256.0,
        0.5,
    ];
    let mut shorts = Vec::new();
    for (variant, length) in lengths.into_iter().enumerate() {
        let ids = window_and_clip(&mut s,
            [short_anchor, [short_anchor[0] + length, short_anchor[1] + length]],
            "diagonal_short_window", "diagonal_short_direct_clip",
            format!("fixed corner anchor=(3,3.5); length_variant={variant}; dx=dy={length}; variant0=exact zero,variant1=adjacent window f32; no snap or coverage assertion"));
        if let Some(&zero) = shorts.first() {
            compare_groups(&mut s, zero, ids, "short_vs_zero_length_observation");
        }
        shorts.push(ids);
    }
    // Direct clip ULP phase sweeps bypass all shader window arithmetic. Both y
    // endpoints have the same exponent, so one ULP each is a common exact shift.
    // Their ideal window values need not both be representable window f32 inputs.
    for (phase_index, phase) in PHASES.into_iter().enumerate() {
        for up in [false, true] {
            let mut points =
                diagonal(0.5, phase).map(|p| project_reference(p, [0.0; 2], FULL).clip);
            for p in &mut points {
                p[1] = if up { next_up(p[1]) } else { next_down(p[1]) };
            }
            let ids = s.pair(points, FULL, 1, "diagonal_clip_ulp",
                format!("fixed direct-clip seed of phase_anchor={phase}; both clip y adjacent_f32_up={up}; common clip ULP preserves exact45; no window shader arithmetic"));
            for direction in 0..2 {
                s.relation(
                    centers[phase_index][1][direction],
                    ids[direction],
                    "direct_clip_phase_ulp_observation",
                    None,
                );
            }
        }
    }
    let a = project_reference(short_anchor, [0.0; 2], FULL).clip;
    let short_clip_ulp = s.pair([a, [next_up(a[0]), next_down(a[1])]], FULL, 1,
        "diagonal_short_clip_ulp", "fixed corner anchor=(3,3.5); one direct clip ULP along each axis; known nonzero exact45 input, not CPU-projected collapse or a snapping model".into());
    for direction in 0..2 {
        s.relation(
            shorts[0][1][direction],
            short_clip_ulp[direction],
            "direct_clip_nonzero_vs_zero_observation",
            None,
        );
        s.relation(
            shorts[1][1][direction],
            short_clip_ulp[direction],
            "collapsed_reference_vs_nonzero_direct_observation",
            None,
        );
    }

    // Retain only reviewed, mechanically justified control TYPES. No expected
    // raster bitmaps, reversal invariance, major-axis choice or corner ownership.
    for source in [centers[4][0][0], centers[4][1][1]] {
        let c = cases::indexed(&s.cases[source], "indexed");
        let id = s.add(
            c,
            "indexed_precision_control",
            format!("shuffled UInt16 equivalent of case {source}"),
        );
        s.relation(source, id, "direct_indexed_control", Some(true));
    }
    for source in [centers[0][0][0], short_clip_ulp[1]] {
        let id = s.add(
            s.cases[source].clone(),
            "repeatability_control",
            format!("identical input of case {source}; separate command buffer"),
        );
        s.relation(source, id, "exact_repeat_control", Some(true));
    }
    let mut clip_controls = Vec::new();
    for source in [32, 34, 36, 38] {
        clip_controls.push(s.clip_companion(source, "direct_clip_exact_control", Some(true)));
    }
    let mut keep = s.cases[clip_controls[0]].clone();
    keep.input.keep = 1;
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
    assert!(s.cases.len() <= CASE_LIMIT);
    assert!(s.cases.len() * 8 * crate::ffi::SLAB < RAW_LIMIT_BYTES);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    fn positions(p: &Input) -> Vec<[f64; 2]> {
        consumed_indices(p)
            .into_iter()
            .map(|i| {
                if p.position_mode == 1 {
                    ideal_window(p.vertices[i], p.viewport)
                } else {
                    p.vertices[i].map(f64::from)
                }
            })
            .collect()
    }
    #[test]
    fn adjacent_binary32_steps_are_exact() {
        for x in [3.0, 4.5, 27.5, -0.75, -0.5, 0.5, 0.75] {
            assert!(next_down(x) < x && next_up(x) > x);
            assert_eq!(next_down(next_up(x)).to_bits(), x.to_bits());
        }
        assert_eq!(next_up(-0.0).to_bits(), 1);
        assert_eq!(next_down(0.0).to_bits(), 0x8000_0001);
    }
    #[test]
    fn preserves_every_baseline_input_tag_and_order() {
        let old = cases::generate();
        let s = generate();
        assert_eq!(old.len(), 66);
        for (a, b) in old.iter().zip(&s.cases) {
            assert_eq!(a.input, b.input);
            assert_eq!(
                (&a.name, a.role, a.family, &a.recipe),
                (&b.name, b.role, b.family, &b.recipe)
            );
        }
    }
    #[test]
    fn generated_inputs_are_deterministic_finite_bounded_and_tagged() {
        let a = generate();
        let b = generate();
        assert_eq!(a.cases.len(), 485);
        assert!(a.cases.len() <= 512);
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
            assert!(consumed_indices(p).iter().all(|&v| v < 8));
            if i >= 66 {
                assert_eq!(p.viewport, FULL);
                assert_eq!(p.scissor, [0, 0, 32, 32]);
                assert_eq!(p.instances, 1);
                assert_eq!(p.offsets, [[0.0; 2]; 3]);
                assert_eq!(p.draw_count, 2);
                assert!(positions(p).iter().flatten().all(|&v| 0.0 < v && v < 32.0));
            }
        }
        assert_eq!(a.pairs.len(), b.pairs.len());
        for (a, b) in a.pairs.iter().zip(&b.pairs) {
            assert_eq!(
                (a.lhs, a.rhs, a.kind, a.expected_equal),
                (b.lhs, b.rhs, b.kind, b.expected_equal)
            );
        }
        assert!(CASE_LIMIT * 8 * crate::ffi::SLAB < RAW_LIMIT_BYTES);
    }
    #[test]
    fn family_counts_are_literal_and_coherent() {
        let s = generate();
        let mut counts = std::collections::BTreeMap::new();
        for c in &s.cases {
            *counts.entry(c.family).or_insert(0) += 1;
        }
        let expected = [
            ("baseline_66", 66),
            ("diagonal_phase_window", 70),
            ("diagonal_phase_direct_clip", 70),
            ("diagonal_symmetry_window", 70),
            ("diagonal_symmetry_direct_clip", 70),
            ("diagonal_start_fraction_window", 18),
            ("diagonal_start_fraction_direct_clip", 18),
            ("diagonal_common_ulp_window", 8),
            ("diagonal_common_ulp_direct_clip", 8),
            ("diagonal_major_neighbor_window", 16),
            ("diagonal_major_neighbor_direct_clip", 16),
            ("diagonal_short_window", 12),
            ("diagonal_short_direct_clip", 12),
            ("diagonal_clip_ulp", 20),
            ("diagonal_short_clip_ulp", 2),
            ("indexed_precision_control", 2),
            ("repeatability_control", 2),
            ("direct_clip_exact_control", 4),
            ("direct_clip_keep_control", 1),
        ]
        .into_iter()
        .collect();
        assert_eq!(counts, expected);
    }
    #[test]
    fn exact_diagonal_families_really_have_equal_absolute_deltas() {
        let s = generate();
        for c in s
            .cases
            .iter()
            .filter(|c| c.family.starts_with("diagonal_") && !c.family.contains("major_neighbor"))
        {
            let p = positions(&c.input);
            assert_eq!(
                (p[1][0] - p[0][0]).abs(),
                (p[1][1] - p[0][1]).abs(),
                "{}",
                c.recipe
            );
        }
        // Arithmetic identity of dyadic INPUTS, never a claim about native masks.
        for pair in s
            .pairs
            .iter()
            .filter(|p| p.kind == "window_vs_cpu_reference_direct_clip_observation")
        {
            let a = &s.cases[pair.lhs];
            if !a.family.contains("major_neighbor") && a.family != "diagonal_short_window" {
                assert_eq!(positions(&a.input), positions(&s.cases[pair.rhs].input));
            }
        }
    }
    #[test]
    fn major_neighbors_bracket_the_tie_without_raster_expectations() {
        let s = generate();
        let mut greater = [0; 2];
        for c in s
            .cases
            .iter()
            .filter(|c| c.family == "diagonal_major_neighbor_window")
        {
            let p = positions(&c.input);
            let d = [(p[1][0] - p[0][0]).abs(), (p[1][1] - p[0][1]).abs()];
            assert_ne!(d[0], d[1]);
            greater[usize::from(d[1] > d[0])] += 1;
        }
        assert_eq!(greater, [8, 8]);
        for p in s
            .pairs
            .iter()
            .filter(|p| p.kind == "near_45_major_axis_neighbor_observation")
        {
            assert_eq!(p.expected_equal, None);
            let a = &s.cases[p.lhs].input;
            let b = &s.cases[p.rhs].input;
            if a.position_mode == 0 {
                let changes = consumed_indices(a)
                    .into_iter()
                    .zip(consumed_indices(b))
                    .map(|(i, j)| {
                        (0..2)
                            .filter(|&k| a.vertices[i][k].to_bits() != b.vertices[j][k].to_bits())
                            .count()
                    })
                    .sum::<usize>();
                assert_eq!(changes, 1);
            }
        }
    }
    #[test]
    fn direct_clip_phase_neighbors_are_adjacent_at_both_endpoints() {
        let s = generate();
        let pairs: Vec<_> = s
            .pairs
            .iter()
            .filter(|p| p.kind == "direct_clip_phase_ulp_observation")
            .collect();
        assert_eq!(pairs.len(), 20);
        for pair in pairs {
            let a = &s.cases[pair.lhs].input;
            let b = &s.cases[pair.rhs].input;
            assert_eq!((a.position_mode, b.position_mode), (1, 1));
            for (i, j) in consumed_indices(a).into_iter().zip(consumed_indices(b)) {
                assert_eq!(a.vertices[i][0].to_bits(), b.vertices[j][0].to_bits());
                assert_eq!(
                    a.vertices[i][1]
                        .to_bits()
                        .abs_diff(b.vertices[j][1].to_bits()),
                    1
                );
            }
            assert_eq!(pair.expected_equal, None);
        }
    }
    #[test]
    fn common_window_step_is_far_endpoint_ulp_not_rounded_away_phase() {
        let s = generate();
        for pair in s.pairs.iter().filter(|p| {
            p.kind == "common_translation_ulp_observation"
                && s.cases[p.lhs].input.position_mode == 0
        }) {
            let a = &s.cases[pair.lhs].input;
            let b = &s.cases[pair.rhs].input;
            let pa = positions(a);
            let pb = positions(b);
            let far = usize::from(pa[1][0] > pa[0][0]);
            assert_ne!(pa[0][1], pb[0][1]);
            assert_eq!(pb[0][1] - pa[0][1], pb[1][1] - pa[1][1]);
            let ai = consumed_indices(a)[far];
            let bi = consumed_indices(b)[far];
            assert_eq!(
                a.vertices[ai][1]
                    .to_bits()
                    .abs_diff(b.vertices[bi][1].to_bits()),
                1
            );
        }
    }
    #[test]
    fn short_nonzero_window_cpu_collapse_and_direct_nonzero_are_distinct() {
        let s = generate();
        let collapsed: Vec<_> = s
            .cases
            .iter()
            .filter(|c| {
                c.family == "diagonal_short_window" && c.recipe.contains("length_variant=1;")
            })
            .collect();
        assert_eq!(collapsed.len(), 2);
        for c in collapsed {
            let p = positions(&c.input);
            assert_ne!(p[0][0], p[1][0]);
            assert_ne!(p[0][1], p[1][1]);
            let reference = reference_clip_bits(&c.input);
            assert_eq!(
                reference[0], reference[1],
                "CPU staged reference only, not GPU or snapping evidence"
            );
        }
        for c in s
            .cases
            .iter()
            .filter(|c| c.family == "diagonal_short_clip_ulp")
        {
            let p = positions(&c.input);
            assert_ne!(p[0][0], p[1][0]);
            assert_ne!(p[0][1], p[1][1]);
            assert_eq!((p[1][0] - p[0][0]).abs(), 1.0 / 1048576.0);
        }
    }
    #[test]
    fn all_reversal_pairs_are_input_reversals_not_equality_claims() {
        let s = generate();
        for p in s
            .pairs
            .iter()
            .filter(|p| p.kind == "endpoint_reversal_observation")
        {
            let a = &s.cases[p.lhs].input;
            let b = &s.cases[p.rhs].input;
            let mut bits = reference_clip_bits(a);
            bits.reverse();
            assert_eq!(bits, reference_clip_bits(b));
            assert_eq!((a.viewport, a.position_mode), (b.viewport, b.position_mode));
            assert_eq!(p.expected_equal, None);
        }
    }
    #[test]
    fn comparison_policy_only_gates_mechanically_justified_controls() {
        let s = generate();
        let mut gated = 0;
        for p in &s.pairs {
            assert!(p.lhs < CASE_COUNT && p.rhs < CASE_COUNT);
            if p.expected_equal.is_some() {
                gated += 1;
                assert!(matches!(
                    p.kind,
                    "exact_dyadic_path_control"
                        | "direct_indexed_control"
                        | "exact_repeat_control"
                        | "direct_clip_keep_wrong_equality_rejected"
                ));
                assert_eq!(
                    reference_clip_bits(&s.cases[p.lhs].input),
                    reference_clip_bits(&s.cases[p.rhs].input)
                );
                if p.kind == "exact_repeat_control" {
                    let a = s.cases[p.lhs].input;
                    let mut b = s.cases[p.rhs].input;
                    b.id = a.id;
                    assert_eq!(a, b);
                }
                if p.kind == "exact_dyadic_path_control" {
                    assert_eq!(
                        positions(&s.cases[p.lhs].input),
                        positions(&s.cases[p.rhs].input)
                    );
                }
            }
        }
        assert_eq!(gated, 9);
    }
}
