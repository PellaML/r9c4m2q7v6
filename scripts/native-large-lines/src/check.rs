use crate::{
    cases::*,
    ffi::{GUARD, ROW, SIDE, SLAB},
};

pub struct Decoded {
    pub mask: Option<Vec<u8>>,
    pub errors: Vec<&'static str>,
}
fn payload(raw: &[u8], slab: usize, bpp: usize) -> Vec<u8> {
    (0..SIDE)
        .flat_map(|y| {
            let start = slab * SLAB + GUARD + y * ROW;
            raw[start..start + SIDE * bpp].iter().copied()
        })
        .collect()
}
fn binary_rgba(bytes: &[u8]) -> Option<Vec<u8>> {
    bytes
        .chunks_exact(4)
        .map(|p| match p {
            [0, 0, 0, 0] => Some(0),
            [255, 255, 255, 255] => Some(1),
            _ => None,
        })
        .collect()
}

pub fn inspect(raw: &[u8], mode: u32) -> Decoded {
    let planes: &[usize] = if mode == 0 {
        &[4, 4, 8, 4, 8, 4]
    } else {
        &[4, 4]
    };
    let mut errors = Vec::new();
    if mode > 1 || raw.len() != planes.len() * SLAB {
        return Decoded {
            mask: None,
            errors: vec!["readback_length_or_mode"],
        };
    }
    for (i, &bpp) in planes.iter().enumerate() {
        let block = &raw[i * SLAB..(i + 1) * SLAB];
        let guards = block[..GUARD].iter().chain(&block[GUARD + ROW * SIDE..]);
        if guards.into_iter().any(|b| *b != 0xa5) && !errors.contains(&"guard_modified") {
            errors.push("guard_modified");
        }
        if (0..SIDE).any(|y| {
            block[GUARD + y * ROW + SIDE * bpp..GUARD + (y + 1) * ROW]
                .iter()
                .any(|b| *b != 0xa5)
        }) && !errors.contains(&"row_padding_modified")
        {
            errors.push("row_padding_modified");
        }
    }
    let mask = binary_rgba(&payload(raw, 0, 4));
    let inverse = binary_rgba(&payload(raw, 1, 4));
    if mask.is_none() {
        errors.push("equal_one_not_binary_rgba");
    }
    if inverse.is_none() {
        errors.push("equal_zero_not_binary_rgba");
    }
    match (&mask, &inverse) {
        (Some(a), Some(b)) if a.iter().zip(b).all(|(a, b)| a + b == 1) => (),
        _ => errors.push("query_complement_failed"),
    }
    if mode == 0 {
        for (before, after, sentinel) in [
            (2, 4, &[0x00, 0x34, 0x00, 0x38, 0x00, 0x3a, 0x00, 0x3c][..]),
            (3, 5, &[0x00, 0x38, 0x00, 0x34][..]),
        ] {
            let a = payload(raw, before, sentinel.len());
            let b = payload(raw, after, sentinel.len());
            if a != b && !errors.contains(&"color_bytes_changed") {
                errors.push("color_bytes_changed");
            }
            if (!a.chunks_exact(sentinel.len()).all(|x| x == sentinel)
                || !b.chunks_exact(sentinel.len()).all(|x| x == sentinel))
                && !errors.contains(&"nonzero_color_sentinel_failed")
            {
                errors.push("nonzero_color_sentinel_failed");
            }
        }
    }
    Decoded { mask, errors }
}

#[derive(Debug)]
pub struct Assertion {
    pub name: String,
    pub passed: bool,
    pub lhs: Option<usize>,
    pub rhs: Option<usize>,
    pub differing_pixels: Option<usize>,
    pub expected_equal: Option<bool>,
}
impl Assertion {
    pub fn simple(name: impl Into<String>, passed: bool) -> Self {
        Self {
            name: name.into(),
            passed,
            lhs: None,
            rhs: None,
            differing_pixels: None,
            expected_equal: None,
        }
    }
}
pub fn differences(a: &[u8], b: &[u8]) -> Option<usize> {
    if a.len() != SIDE * SIDE || b.len() != SIDE * SIDE {
        return None;
    }
    Some(a.iter().zip(b).filter(|(x, y)| x != y).count())
}
fn comparison(
    name: String,
    a: Option<&Vec<u8>>,
    b: Option<&Vec<u8>>,
    ids: (usize, usize),
    equal: bool,
) -> Assertion {
    let count = a.zip(b).and_then(|(a, b)| differences(a, b));
    Assertion {
        name,
        passed: count.is_some_and(|n| (n == 0) == equal),
        lhs: Some(ids.0),
        rhs: Some(ids.1),
        differing_pixels: count,
        expected_equal: Some(equal),
    }
}

// A mask here is a checked native observation, not a geometry-derived prediction.
pub fn relations(cases: &[Case], masks: &[Vec<Option<Vec<u8>>>]) -> Vec<Assertion> {
    let mut checks = Vec::new();
    for (mode, mode_masks) in masks.iter().enumerate() {
        checks.push(Assertion::simple(
            format!("mode_{mode}_nonempty_line_positive"),
            mode_masks[BASE]
                .as_ref()
                .is_some_and(|m| m.len() == SIDE * SIDE && m.contains(&1)),
        ));
        for id in [KEEP, CLEAR_ZERO] {
            checks.push(Assertion::simple(
                format!("mode_{mode}_empty_control_{id}"),
                mode_masks[id]
                    .as_ref()
                    .is_some_and(|m| m.len() == SIDE * SIDE && m.iter().all(|v| *v == 0)),
            ));
        }
        checks.push(Assertion::simple(
            format!("mode_{mode}_query_full_one"),
            mode_masks[CLEAR_ONE]
                .as_ref()
                .is_some_and(|m| m.len() == SIDE * SIDE && m.iter().all(|v| *v == 1)),
        ));
    }
    for (id, _) in cases.iter().enumerate() {
        checks.push(comparison(
            format!("attachment_layout_equivalence_{id}"),
            masks[0][id].as_ref(),
            masks[1][id].as_ref(),
            (id, id),
            true,
        ));
    }
    checks
}

pub fn qualifies(native: bool, paravirtual: bool, count: usize, checks: &[Assertion]) -> bool {
    native
        && paravirtual
        && count == crate::extension::MEASUREMENT_COUNT
        && !checks.is_empty()
        && checks.iter().all(|c| c.passed)
}

pub fn extension_controls(
    s: &crate::extension::Suite,
    masks: &[Vec<Option<Vec<u8>>>],
) -> Vec<Assertion> {
    let mut checks = Vec::new();
    for (mode, mode_masks) in masks.iter().enumerate() {
        for p in s.pairs.iter().filter(|p| p.expected_equal.is_some()) {
            checks.push(comparison(
                format!("mode_{mode}_{}_{}_{}", p.kind, p.lhs, p.rhs),
                mode_masks[p.lhs].as_ref(),
                mode_masks[p.rhs].as_ref(),
                (p.lhs, p.rhs),
                p.expected_equal.unwrap(),
            ));
        }
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    // Decoder-only fabricated bytes. Not exposed as a runnable or native evidence path.
    fn fixture(mode: u32) -> Vec<u8> {
        let mut raw = vec![0xa5; if mode == 0 { 6 * SLAB } else { 2 * SLAB }];
        for y in 0..SIDE {
            for x in 0..SIDE {
                for (slab, pixel) in [
                    (0, &[0, 0, 0, 0][..]),
                    (1, &[255, 255, 255, 255][..]),
                    (2, &[0, 0x34, 0, 0x38, 0, 0x3a, 0, 0x3c][..]),
                    (3, &[0, 0x38, 0, 0x34][..]),
                    (4, &[0, 0x34, 0, 0x38, 0, 0x3a, 0, 0x3c][..]),
                    (5, &[0, 0x38, 0, 0x34][..]),
                ]
                .into_iter()
                .take(if mode == 0 { 6 } else { 2 })
                {
                    let start = slab * SLAB + GUARD + y * ROW + x * pixel.len();
                    raw[start..start + pixel.len()].copy_from_slice(pixel);
                }
            }
        }
        raw
    }
    #[test]
    fn decoder_accepts_only_well_formed_readbacks() {
        for mode in 0..=1 {
            let d = inspect(&fixture(mode), mode);
            assert!(d.errors.is_empty());
        }
    }
    #[test]
    fn all_zeros_poison_and_complement_faults_fail() {
        for raw in [
            vec![0; 2 * SLAB],
            vec![0xa5; 2 * SLAB],
            vec![0xde; 2 * SLAB],
        ] {
            assert!(!inspect(&raw, 1).errors.is_empty());
        }
        let mut raw = fixture(1);
        raw[SLAB + GUARD..SLAB + GUARD + 4].fill(0);
        assert!(inspect(&raw, 1).errors.contains(&"query_complement_failed"));
    }
    #[test]
    fn guard_padding_and_color_mutations_fail() {
        for (position, reason) in [
            (0, "guard_modified"),
            (GUARD + SIDE * 4, "row_padding_modified"),
            (4 * SLAB + GUARD, "color_bytes_changed"),
        ] {
            let mut raw = fixture(0);
            raw[position] ^= 1;
            assert!(inspect(&raw, 0).errors.contains(&reason));
        }
    }
    #[test]
    fn full_extent_decoder_reaches_translated_and_last_pixels() {
        for mode in 0..=1 {
            let mut raw = fixture(mode);
            let pixels = [(0, 0), (199, 200), (239, 217), (255, 255)];
            for (x, y) in pixels {
                let offset = GUARD + y * ROW + x * 4;
                raw[offset..offset + 4].fill(255);
                raw[SLAB + offset..SLAB + offset + 4].fill(0);
            }
            let decoded = inspect(&raw, mode);
            assert!(decoded.errors.is_empty());
            let mask = decoded.mask.unwrap();
            assert_eq!(mask.len(), 65536);
            assert_eq!(mask.iter().filter(|&&v| v == 1).count(), 4);
            for (x, y) in pixels {
                assert_eq!(mask[y * SIDE + x], 1);
            }
        }
    }
    #[test]
    fn every_slab_guard_and_last_row_padding_are_checked() {
        let raw = fixture(0);
        for (slab, bpp) in [4, 4, 8, 4, 8, 4].into_iter().enumerate() {
            for offset in [slab * SLAB, (slab + 1) * SLAB - 1] {
                let mut bad = raw.clone();
                bad[offset] ^= 1;
                assert!(inspect(&bad, 0).errors.contains(&"guard_modified"));
            }
            if bpp * SIDE < ROW {
                let mut bad = raw.clone();
                bad[slab * SLAB + GUARD + (SIDE - 1) * ROW + SIDE * bpp] ^= 1;
                assert!(inspect(&bad, 0).errors.contains(&"row_padding_modified"));
            }
        }
    }
    #[test]
    fn final_color_pixel_and_nonbinary_query_channels_are_checked() {
        for slab in [2, 3, 4, 5] {
            let mut raw = fixture(0);
            let bpp = if slab % 2 == 0 { 8 } else { 4 };
            raw[slab * SLAB + GUARD + (SIDE - 1) * ROW + SIDE * bpp - 1] ^= 1;
            let d = inspect(&raw, 0);
            assert!(d.errors.contains(&"color_bytes_changed"));
            assert!(d.errors.contains(&"nonzero_color_sentinel_failed"));
        }
        let mut raw = fixture(1);
        raw[GUARD + (SIDE - 1) * ROW + (SIDE - 1) * 4 + 3] = 1;
        assert!(inspect(&raw, 1)
            .errors
            .contains(&"equal_one_not_binary_rgba"));
        assert!(inspect(&raw[..raw.len() - 1], 1)
            .errors
            .contains(&"readback_length_or_mode"));
        assert!(inspect(&raw, 2).errors.contains(&"readback_length_or_mode"));
    }
    #[test]
    fn identical_zero_colors_are_not_evidence_of_no_writes() {
        let mut raw = fixture(0);
        for slab in 2..6 {
            for y in 0..SIDE {
                let bpp = if slab % 2 == 0 { 8 } else { 4 };
                raw[slab * SLAB + GUARD + y * ROW..slab * SLAB + GUARD + y * ROW + SIDE * bpp]
                    .fill(0);
            }
        }
        assert!(inspect(&raw, 0)
            .errors
            .contains(&"nonzero_color_sentinel_failed"));
    }
    #[test]
    fn all_empty_or_all_one_observations_cannot_qualify() {
        let s = crate::extension::generate();
        for value in [0, 1] {
            let masks = vec![vec![Some(vec![value; SIDE * SIDE]); CASE_COUNT]; 2];
            let mut checks = relations(&s.cases, &masks);
            checks.extend(extension_controls(&s, &masks));
            assert!(!qualifies(true, true, 2 * CASE_COUNT, &checks));
            assert!(checks
                .iter()
                .filter(|a| a.name.contains("wrong_keep"))
                .all(|a| !a.passed));
        }
    }
    #[test]
    fn partial_no_device_and_wrong_device_never_qualify() {
        let checks = [Assertion::simple("checked", true)];
        assert!(!qualifies(false, true, 2 * CASE_COUNT, &checks));
        assert!(!qualifies(true, false, 2 * CASE_COUNT, &checks));
        assert!(!qualifies(true, true, 2 * CASE_COUNT - 1, &checks));
        assert!(!qualifies(true, true, 2 * CASE_COUNT, &[]));
    }
    #[test]
    fn exact_comparator_rejects_a_single_changed_pixel() {
        let a = vec![0; SIDE * SIDE];
        let mut b = a.clone();
        b[17] = 1;
        assert_eq!(differences(&a, &b), Some(1));
        assert_eq!(differences(&a, &a), Some(0));
        assert_eq!(differences(&a[..1], &b), None);
    }

    #[test]
    fn missing_or_zero_readbacks_cannot_qualify() {
        let s = crate::extension::generate();
        for entry in [Some(vec![0; SIDE * SIDE]), None] {
            let masks = vec![vec![entry; s.cases.len()]; 2];
            let mut checks = relations(&s.cases, &masks);
            checks.extend(extension_controls(&s, &masks));
            assert!(!qualifies(
                true,
                true,
                crate::extension::MEASUREMENT_COUNT,
                &checks
            ));
            assert!(checks
                .iter()
                .any(|c| c.name.contains("wrong_keep") && !c.passed));
        }
    }
}
