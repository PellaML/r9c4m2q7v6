use crate::ffi::{Input, SIDE};

pub const CASE_COUNT: usize = 16;
pub const CLEAR_ZERO: usize = 0;
pub const CLEAR_ONE: usize = 1;
pub const BASE: usize = 2;
pub const REPEAT: usize = 3;
pub const INDEXED: usize = 4;
pub const KEEP: usize = 5;

#[derive(Clone)]
pub struct Case {
    pub name: String,
    pub role: &'static str,
    pub input: Input,
    pub family: &'static str,
    pub recipe: String,
    // Authored integer numerators, in draw order; denominator is exactly 256.
    // These describe inputs only, never predicted covered pixels.
    pub window_grid_256: Vec<[i32; 2]>,
}

pub fn clip_from_grid(p: [i32; 2]) -> [f32; 2] {
    // SIDE=256: all integers and these power-of-two operations are exact.
    [p[0] as f32 / 32768.0 - 1.0, 1.0 - p[1] as f32 / 32768.0]
}

fn direct(name: &str, family: &'static str, grid: &[[i32; 2]]) -> Case {
    assert!(grid.len() <= 6);
    let mut input = Input {
        id: 0,
        draw_count: grid.len() as u32,
        vertex_start: 1,
        index_first: 0,
        indexed: 0,
        instances: 1,
        primitive: 0,
        keep: 0,
        clear_stencil: 0,
        no_draw: 0,
        position_mode: 1,
        vertices: [[-4.0, -4.0]; 8],
        indices: [7; 10],
        offsets: [[0.0; 2]; 3],
        viewport: [0.0, 0.0, SIDE as f32, SIDE as f32],
        scissor: [0, 0, SIDE as u32, SIDE as u32],
    };
    for (i, &p) in grid.iter().enumerate() {
        assert!(p.into_iter().all(|n| (0..=65536).contains(&n)));
        input.vertices[i + 1] = clip_from_grid(p);
    }
    Case {
        name: name.into(),
        role: "observation",
        input,
        family,
        recipe: "independently authored grid numerators / 256; exact clip xy=(grid_x/32768-1,1-grid_y/32768); identity vertex entry, z=0.5,w=1".into(),
        window_grid_256: grid.to_vec(),
    }
}

fn indexed(c: &Case) -> Case {
    let mut result = c.clone();
    result.name = "positive_line_uint16_shuffled".into();
    result.role = "indexed_equivalence_control";
    let p = &mut result.input;
    p.vertices = [[-4.0, -4.0]; 8];
    p.indexed = 1;
    p.index_first = 1;
    p.vertex_start = 0;
    // Nonzero index-buffer byte offset 2, poisoned prefix and shuffled storage.
    for (i, slot) in [2, 5].into_iter().enumerate() {
        p.vertices[slot] = c.input.vertices[c.input.vertex_start as usize + i];
        p.indices[1 + i] = slot as u16;
    }
    result
}

pub fn generate() -> Vec<Case> {
    let mut c = Vec::with_capacity(CASE_COUNT);
    for value in 0..=1 {
        let mut cal = direct(
            &format!("query_clear_{value}_no_draw"),
            "query_calibration",
            &[],
        );
        cal.role = "query_calibration";
        cal.input.no_draw = 1;
        cal.input.clear_stencil = value;
        c.push(cal);
    }
    let mut positive = direct(
        "positive_vertical_line",
        "positive_and_controls",
        &[[50048, 43392], [50048, 59776]], // (195.5,169.5) -> (195.5,233.5)
    );
    positive.role = "nonempty_line_positive_control";
    c.push(positive.clone());
    let mut repeat = positive.clone();
    repeat.name = "positive_line_exact_repeat".into();
    repeat.role = "exact_repeat_control";
    c.push(repeat);
    c.push(indexed(&positive));
    let mut keep = positive;
    keep.name = "positive_line_keep_negative".into();
    keep.role = "keep_negative_control";
    keep.input.keep = 1;
    c.push(keep);

    // Five new generic pairs, all comfortably inside the target, all >32 pixels.
    // Reversal is diagnostic ONLY: no endpoint ownership or mask equality assumed.
    for (family, points) in [
        ("long_horizontal", [[44352, 52225], [60736, 52225]]),
        ("oblique_shallow", [[45220, 45973], [60216, 55052]]),
        ("oblique_steep_negative", [[56405, 45287], [47208, 60186]]),
        ("near_horizontal", [[45152, 50943], [61536, 50945]]),
        ("near_vertical", [[54529, 43104], [54527, 59488]]),
    ] {
        for direction in 0..2 {
            let grid = if direction == 0 {
                points
            } else {
                [points[1], points[0]]
            };
            let mut case = direct(&format!("{family}_direction_{direction}"), family, &grid);
            case.recipe.push_str(&format!(
                "; direction={direction}; reversal has no coverage expectation"
            ));
            c.push(case);
        }
    }
    for (id, case) in c.iter_mut().enumerate() {
        case.input.id = id as u32;
    }
    assert_eq!(c.len(), CASE_COUNT);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{consumed_indices, ideal_window, reference_clip_bits};

    #[test]
    fn bounded_small_set_has_only_exact_identity_geometry() {
        let c = generate();
        assert_eq!(c.len(), 16);
        for (id, case) in c.iter().enumerate() {
            let p = &case.input;
            assert_eq!(p.id as usize, id);
            assert_eq!(p.position_mode, 1);
            assert_eq!(p.instances, 1);
            assert_eq!(p.offsets, [[0.0; 2]; 3]);
            assert_eq!(p.primitive, 0);
            assert_eq!(p.scissor, [0, 0, 256, 256]);
            assert_eq!(p.viewport, [0.0, 0.0, 256.0, 256.0]);
            assert_eq!(p.draw_count as usize, case.window_grid_256.len());
            for (i, vid) in consumed_indices(p).into_iter().enumerate() {
                assert!(vid < 8);
                assert_eq!(
                    ideal_window(p.vertices[vid], p.viewport),
                    case.window_grid_256[i].map(|n| f64::from(n) / 256.0)
                );
                assert!(p.vertices[vid].into_iter().all(|v| v.abs() < 1.0));
            }
            if id >= BASE {
                let [a, b] = [case.window_grid_256[0], case.window_grid_256[1]];
                let dx = f64::from(b[0] - a[0]) / 256.0;
                let dy = f64::from(b[1] - a[1]) / 256.0;
                assert!(dx.hypot(dy) > 32.0);
                assert!(a
                    .into_iter()
                    .chain(b)
                    .all(|n| (168 * 256..241 * 256).contains(&n)));
            }
        }
        for id in [CLEAR_ZERO, CLEAR_ONE] {
            assert_eq!(c[id].input.no_draw, 1);
            assert_eq!(c[id].input.draw_count, 0);
            assert_eq!(c[id].input.clear_stencil, id as u32);
        }
    }

    #[test]
    fn controls_are_real_identical_repeat_indexing_and_keep_mutation() {
        let c = generate();
        let mut repeat = c[REPEAT].input;
        repeat.id = c[BASE].input.id;
        assert_eq!(repeat, c[BASE].input);
        assert_eq!(
            reference_clip_bits(&c[BASE].input),
            reference_clip_bits(&c[INDEXED].input)
        );
        assert_eq!(c[INDEXED].input.index_first, 1);
        assert_ne!(c[INDEXED].input.vertices, c[BASE].input.vertices);
        let mut keep = c[KEEP].input;
        keep.id = c[BASE].input.id;
        assert_eq!(keep.keep, 1);
        keep.keep = 0;
        assert_eq!(keep, c[BASE].input);
    }

    #[test]
    fn every_observation_family_has_both_endpoint_orders() {
        let c = generate();
        for first in (6..CASE_COUNT).step_by(2) {
            assert_eq!(c[first].family, c[first + 1].family);
            assert_eq!(
                c[first].window_grid_256,
                c[first + 1]
                    .window_grid_256
                    .iter()
                    .rev()
                    .copied()
                    .collect::<Vec<_>>()
            );
            let mut bits = reference_clip_bits(&c[first + 1].input);
            bits.reverse();
            assert_eq!(reference_clip_bits(&c[first].input), bits);
        }
    }
}
