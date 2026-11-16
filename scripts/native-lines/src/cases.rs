use crate::ffi::Input;

pub const CASE_COUNT: usize = 66;
pub const BASE: usize = 56;
pub const INDEXED: usize = 57;
pub const INSTANCED: usize = 58;
pub const INSTANCED_INDEXED: usize = 59;
pub const KEEP: usize = 60;
pub const MOVED: usize = 61;
pub const TRIANGLE: usize = 62;
pub const EXPANDED: usize = 63;
pub const CLEAR_ZERO: usize = 64;
pub const CLEAR_ONE: usize = 65;

#[derive(Clone)]
pub struct Case {
    pub name: String,
    pub role: &'static str,
    pub input: Input,
}

fn direct(name: &str, points: &[[f32; 2]]) -> Case {
    assert!(points.len() <= 6);
    let mut p = Input {
        id: 0,
        draw_count: points.len() as u32,
        vertex_start: 1,
        index_first: 0,
        indexed: 0,
        instances: 1,
        primitive: 0,
        keep: 0,
        clear_stencil: 0,
        no_draw: 0,
        vertices: [[-96.0, -96.0]; 8],
        indices: [7; 10],
        offsets: [[0.0; 2]; 3],
        viewport: [0.0, 0.0, 32.0, 32.0],
        scissor: [0, 0, 32, 32],
    };
    p.vertices[1..1 + points.len()].copy_from_slice(points);
    Case {
        name: name.into(),
        role: "observation",
        input: p,
    }
}
fn indexed(c: &Case, name: &str) -> Case {
    let mut result = c.clone();
    result.name = name.into();
    let p = &mut result.input;
    p.vertices = [[-96.0, -96.0]; 8];
    p.indexed = 1;
    p.index_first = 1;
    p.vertex_start = 0;
    // Deliberately shuffled vertex storage, a poison prefix, UInt16 byte offset 2.
    for (i, slot) in [2, 5, 1, 6, 3, 4]
        .into_iter()
        .take(p.draw_count as usize)
        .enumerate()
    {
        p.vertices[slot] = c.input.vertices[c.input.vertex_start as usize + i];
        p.indices[1 + i] = slot as u16;
    }
    result
}

pub fn generate() -> Vec<Case> {
    let mut c = Vec::with_capacity(CASE_COUNT);
    let deltas = [
        [10.0, 4.0],
        [4.0, 10.0],
        [-4.0, 10.0],
        [-10.0, 4.0],
        [-10.0, -4.0],
        [-4.0, -10.0],
        [4.0, -10.0],
        [10.0, -4.0],
    ];
    for (octant, d) in deltas.into_iter().enumerate() {
        for (phase, f) in [0.0, 0.25, 0.5, 0.75].into_iter().enumerate() {
            let a = [16.0 + f, 16.0 + f];
            c.push(direct(
                &format!("octant_{octant}_quarter_{phase}"),
                &[a, [a[0] + d[0], a[1] + d[1]]],
            ));
        }
    }
    for (name, a, b) in [
        ("horizontal", [4.5, 8.5], [27.5, 8.5]),
        ("vertical", [9.5, 4.5], [9.5, 27.5]),
        ("diagonal_down", [4.5, 4.5], [27.5, 27.5]),
        ("diagonal_up", [4.5, 27.5], [27.5, 4.5]),
    ] {
        c.push(direct(&format!("{name}_forward"), &[a, b]));
        c.push(direct(&format!("{name}_reverse"), &[b, a]));
    }
    for (phase, f) in [0.0, 0.25, 0.5, 0.75].into_iter().enumerate() {
        c.push(direct(
            &format!("zero_length_quarter_{phase}"),
            &[[14.0 + f, 18.0 + f]; 2],
        ));
    }
    for (name, p) in [
        ("tie_integer_2_to_1", [[3.0, 3.0], [27.0, 15.0]]),
        ("tie_half_1_to_2", [[4.5, 3.5], [16.5, 27.5]]),
        ("mixed_endpoint_quarters", [[3.25, 25.75], [27.75, 13.25]]),
        ("mixed_endpoint_integer_half", [[27.5, 26.0], [3.0, 13.5]]),
    ] {
        c.push(direct(name, &p));
    }
    for base in [32, 34, 36, 38] {
        let mut s = c[base].clone();
        s.name = format!("scissor_of_{base}");
        s.role = "scissor_equivalence";
        s.input.scissor = [8, 6, 14, 18];
        c.push(s);
    }
    c.push(direct(
        "clip_horizontal_both_ends",
        &[[-8.5, 14.5], [40.5, 14.5]],
    ));
    c.push(direct(
        "clip_diagonal_both_ends",
        &[[-8.25, -8.25], [40.75, 40.75]],
    ));
    let mut inset = direct("inset_viewport_clip", &[[-4.0, 8.0], [36.0, 24.0]]);
    inset.input.viewport = [8.0, 8.0, 16.0, 16.0];
    c.push(inset);
    let mut off = direct("fully_clipped_left", &[[-8.5, 4.5], [-2.5, 27.5]]);
    off.role = "empty_clip_control";
    c.push(off);
    assert_eq!(c.len(), BASE);
    let mut batch = direct(
        "batch_direct",
        &[
            [3.5, 3.5],
            [26.5, 3.5],
            [26.5, 22.5],
            [3.5, 22.5],
            [5.5, 10.5],
            [24.5, 15.5],
        ],
    );
    batch.role = "positive_control";
    c.push(batch.clone());
    c.push(indexed(&batch, "batch_ushort_shuffled"));
    let mut inst = direct("instances_three_direct", &[[3.25, 3.5], [21.75, 6.5]]);
    inst.input.instances = 3;
    inst.input.offsets = [[0.0, 0.0], [0.0, 7.0], [4.0, 14.0]];
    c.push(inst.clone());
    c.push(indexed(&inst, "instances_three_ushort"));
    let mut keep = batch.clone();
    keep.name = "wrong_stencil_keep".into();
    keep.role = "negative_control";
    keep.input.keep = 1;
    c.push(keep);
    let mut moved = batch.clone();
    moved.name = "wrong_positions_shifted".into();
    moved.role = "negative_control";
    for v in &mut moved.input.vertices[1..7] {
        v[1] += 5.0;
    }
    c.push(moved);
    let mut tri = batch;
    tri.name = "wrong_triangle_primitive".into();
    tri.role = "negative_control";
    tri.input.primitive = 1;
    c.push(tri);
    let mut expanded = Vec::new();
    for offset in inst.input.offsets {
        for p in &inst.input.vertices[1..3] {
            expanded.push([p[0] + offset[0], p[1] + offset[1]]);
        }
    }
    c.push(direct("instances_expanded_native_lines", &expanded));
    for value in 0..=1 {
        let mut cal = direct(&format!("query_clear_{value}_no_draw"), &[]);
        cal.role = "query_calibration";
        cal.input.no_draw = 1;
        cal.input.clear_stencil = value;
        c.push(cal);
    }
    for (i, spec) in c.iter_mut().enumerate() {
        spec.input.id = i as u32;
    }
    assert_eq!(c.len(), CASE_COUNT);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    fn consumed(p: &Input) -> Vec<[f32; 2]> {
        (0..p.draw_count as usize)
            .map(|i| {
                p.vertices[if p.indexed != 0 {
                    p.indices[p.index_first as usize + i] as usize
                } else {
                    p.vertex_start as usize + i
                }]
            })
            .collect()
    }
    #[test]
    fn suite_is_finite_and_all_indices_are_in_bounds() {
        let c = generate();
        assert_eq!(c.len(), 66);
        for (i, spec) in c.iter().enumerate() {
            let p = &spec.input;
            assert_eq!(p.id as usize, i);
            assert!(p.instances <= 3);
            assert!(p.draw_count <= 6);
            assert!(consumed(p).len() <= 6);
            assert!(p.scissor[0] + p.scissor[2] <= 32);
            assert!(p.scissor[1] + p.scissor[3] <= 32);
            assert_eq!(p.draw_count % if p.primitive == 1 { 3 } else { 2 }, 0);
        }
    }
    #[test]
    fn indexed_inputs_really_match_direct() {
        let c = generate();
        for (a, b) in [(BASE, INDEXED), (INSTANCED, INSTANCED_INDEXED)] {
            assert_eq!(consumed(&c[a].input), consumed(&c[b].input));
            assert_eq!(c[b].input.index_first, 1);
            assert_ne!(c[a].input.vertices, c[b].input.vertices);
        }
    }
    #[test]
    fn expanded_input_matches_instance_offsets_not_a_coverage_model() {
        let c = generate();
        let p = c[INSTANCED].input;
        let expected: Vec<_> = p
            .offsets
            .into_iter()
            .flat_map(|d| {
                consumed(&p)
                    .into_iter()
                    .map(move |v| [v[0] + d[0], v[1] + d[1]])
            })
            .collect();
        assert_eq!(expected, consumed(&c[EXPANDED].input));
    }
    #[test]
    fn negative_inputs_are_real_mutations() {
        let c = generate();
        assert_eq!(consumed(&c[KEEP].input), consumed(&c[BASE].input));
        assert_eq!(c[KEEP].input.keep, 1);
        assert_ne!(consumed(&c[MOVED].input), consumed(&c[BASE].input));
        assert_eq!(consumed(&c[TRIANGLE].input), consumed(&c[BASE].input));
        assert_eq!(c[TRIANGLE].input.primitive, 1);
    }
}
