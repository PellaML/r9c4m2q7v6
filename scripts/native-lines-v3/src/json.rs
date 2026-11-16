use crate::{
    cases::Case,
    check::Assertion,
    extension::{self, Pair, Suite},
    ffi::{self, Input, Metadata, Receipt},
};

pub fn q(text: &str) -> String {
    let mut s = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if c <= '\u{1f}' => s.push_str(&format!("\\u{:04x}", c as u32)),
            _ => s.push(c),
        }
    }
    s.push('"');
    s
}
pub fn array<T: ToString>(items: impl IntoIterator<Item = T>) -> String {
    format!(
        "[{}]",
        items
            .into_iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn finite(n: f64) -> String {
    if n.is_finite() {
        n.to_string()
    } else {
        "null".into()
    }
}
pub fn receipt(r: &Receipt) -> String {
    format!("{{\"submitted\":{},\"completion_handler_observed\":{},\"timed_out\":{},\"status_raw\":{},\"error_present\":{},\"error_code\":{},\"error_domain\":{},\"reason\":{},\"readback_bytes\":{},\"mode\":{},\"gpu_start_seconds\":{},\"gpu_end_seconds\":{}}}",
        r.submitted == 1, r.completion_seen == 1, r.timed_out != 0, r.status,
        r.error_present != 0, r.error_code, q(&ffi::cstr(&r.error_domain)), q(&ffi::cstr(&r.reason)),
        r.bytes, r.mode, finite(r.gpu_start), finite(r.gpu_end))
}
pub fn metadata(m: &Metadata) -> String {
    format!("{{\"device_name\":{},\"registry_id_hex\":\"{:016x}\",\"paravirtual_name_match\":{},\"identity_basis\":\"public MTLDevice.name contains Apple and Paravirtual, case insensitive; not independent attestation\",\"unified_memory\":{},\"low_power\":{},\"removable\":{},\"device_count\":{},\"selection_raw\":{},\"available_modes_mask\":{},\"os_version\":{},\"os_build\":{},\"architecture\":{},\"clang_version\":{},\"sdk_max_macro\":{},\"sdk_version\":{},\"sdk_build\":{},\"rustc\":{},\"target\":{},\"mode_initialization_errors\":{}}}",
        q(&ffi::cstr(&m.device_name)), m.registry_id, m.paravirtual_name_match == 1,
        m.unified_memory != 0, m.low_power != 0, m.removable != 0, m.device_count, m.selected_by,
        m.available_modes, q(&ffi::cstr(&m.os_version)), q(&ffi::cstr(&m.os_build)),
        q(&ffi::cstr(&m.architecture)), q(&ffi::cstr(&m.clang_version)), m.sdk_max,
        q(env!("PROBE_SDK_VERSION")), q(env!("PROBE_SDK_BUILD")), q(env!("PROBE_RUSTC")),
        q(env!("PROBE_TARGET")), array(m.mode_errors.iter().map(receipt)))
}
fn points(p: &[[f32; 2]]) -> String {
    array(p.iter().map(|p| array(p.iter())))
}
fn point_bits(p: &[[f32; 2]]) -> String {
    array(
        p.iter()
            .map(|p| array(p.iter().map(|v| q(&format!("{:08x}", v.to_bits()))))),
    )
}
pub fn input(p: &Input) -> String {
    format!("{{\"id\":{},\"vertex_storage_xy\":{},\"vertex_storage_f32_bits\":{},\"vertex_start\":{},\"draw_count\":{},\"indexed_uint16\":{},\"indices_storage\":{},\"index_first\":{},\"index_buffer_offset_bytes\":{},\"instances\":{},\"instance_offsets_xy\":{},\"instance_offsets_f32_bits\":{},\"base_vertex\":0,\"base_instance\":0,\"viewport_xywh\":{},\"scissor_xywh\":{},\"primitive\":{},\"stencil_pass\":{},\"clear_stencil\":{},\"no_draw\":{},\"position_mode\":{},\"vertex_coordinate_space\":{},\"viewport_f32_bits\":{}}}",
        p.id, points(&p.vertices), point_bits(&p.vertices), p.vertex_start, p.draw_count,
        p.indexed != 0, array(p.indices.iter()), p.index_first, p.index_first * 2,
        p.instances, points(&p.offsets), point_bits(&p.offsets), array(p.viewport.iter()),
        array(p.scissor.iter()), q(if p.primitive == 0 { "line" } else { "triangle_negative_control" }),
        q(if p.keep == 0 { "Replace" } else { "Keep_negative_control" }), p.clear_stencil, p.no_draw != 0,
        p.position_mode, q(if p.position_mode == 0 { "nominal_window_xy" } else { "direct_clip_xy" }),
        bits(&p.viewport))
}
pub fn cases(c: &[Case]) -> String {
    format!("{{\"schema\":\"generic-native-lines-inputs-v2\",\"case_count\":{},\"width\":32,\"height\":32,\"samples\":1,\"z\":0.5,\"w\":1,\"pixel_origin\":\"top-left, x right, y down\",\"position_transform_applies_to\":\"position_mode=0 only; mode=1 loads clip xy directly\",\"native_vertex_readback\":false,\"fast_math_enabled\":false,\"position_transform\":\"((vertex_xy + instance_offset - viewport_xy) / viewport_wh) * (2,-2) + (-1,1)\",\"test_pipeline\":{{\"fragment_function\":null,\"rasterization_enabled\":true,\"depth_format\":\"Invalid\",\"depth_compare\":\"Always\",\"depth_write\":false,\"stencil_format\":\"Stencil8\",\"stencil_compare_front_back\":\"Always\",\"stencil_reference_front_back\":1,\"stencil_read_mask\":255,\"stencil_write_mask\":255,\"stencil_fail\":\"Keep\",\"depth_fail\":\"Keep\",\"pass\":\"Replace unless negative control\",\"alpha_to_coverage\":false,\"cull\":\"None\",\"depth_clip\":\"Clip\",\"stencil_store\":\"Store\"}},\"attachment_layouts\":[{{\"mode\":0,\"color_formats\":[\"RGBA16Float\",\"RG16Float\"],\"color_write_masks\":[0,0]}},{{\"mode\":1,\"color_formats\":[],\"color_write_masks\":[]}}],\"cases\":{}}}\n",
        c.len(), array(c.iter().map(|c| format!("{{\"name\":{},\"role\":{},\"input\":{},\"family\":{},\"recipe\":{},\"projection_reference\":{}}}", q(&c.name), q(c.role), input(&c.input), q(c.family), q(&c.recipe), projection_reference(&c.input)))))
}

fn bits(values: &[f32]) -> String {
    array(values.iter().map(|v| q(&format!("{:08x}", v.to_bits()))))
}
pub fn projection_reference(p: &Input) -> String {
    let mut records = Vec::new();
    for iid in 0..p.instances as usize {
        for vid in extension::consumed_indices(p) {
            let value = if p.position_mode == 1 {
                format!("{{\"basis\":\"direct_clip_input_identity_expression_not_vertex_readback\",\"clip_input_xy_f32_bits\":{},\"ideal_window_xy_f64\":{}}}",
                    bits(&p.vertices[vid]), array(extension::ideal_window(p.vertices[vid], p.viewport)))
            } else {
                let r = extension::project_reference(p.vertices[vid], p.offsets[iid], p.viewport);
                format!("{{\"basis\":\"CPU_staged_binary32_reference_NOT_native_vertex_bits\",\"after_offset_bits\":{},\"relative_bits\":{},\"divided_bits\":{},\"scaled_bits\":{},\"clip_xy_reference_bits\":{},\"ideal_window_from_reference_f64\":{}}}",
                    bits(&r.after_offset), bits(&r.relative), bits(&r.divided), bits(&r.scaled), bits(&r.clip), array(r.ideal_window))
            };
            records.push(format!(
                "{{\"instance_id\":{iid},\"vertex_id\":{vid},\"arithmetic\":{value}}}"
            ));
        }
    }
    format!("{{\"native_vertex_readback\":false,\"z_bits\":\"3f000000\",\"w_bits\":\"3f800000\",\"warning\":\"Window references assume staged f32 rounding; shader contraction or evaluation may differ even with fastMathEnabled=NO. Ideal window values are f64 reference calculations, not hardware viewport output. No raster quantization is inferred.\",\"vertices\":{}}}", array(records))
}
pub fn pair_plan(p: &Pair) -> String {
    format!(
        "{{\"lhs_case\":{},\"rhs_case\":{},\"kind\":{},\"expected_equal\":{},\"gating\":{}}}",
        p.lhs,
        p.rhs,
        q(p.kind),
        option(p.expected_equal),
        p.expected_equal.is_some()
    )
}
pub fn generator(s: &Suite) -> String {
    let mut families = std::collections::BTreeMap::<&str, Vec<usize>>::new();
    for (id, c) in s.cases.iter().enumerate() {
        families.entry(c.family).or_default().push(id);
    }
    let groups = families.into_iter().map(|(family, ids)| {
        format!(
            "{{\"family\":{},\"count\":{},\"case_ids\":{}}}",
            q(family),
            ids.len(),
            array(ids)
        )
    });
    format!("{{\"schema\":\"generic-line-generator-v3\",\"case_count\":{},\"baseline_cases\":66,\"measurement_count\":{},\"rng\":\"none\",\"recipe\":\"fixed_diagonal_dyadic_grid_and_adjacent_f32_v3\",\"case_limit_inclusive\":{},\"raw_readback_bytes_full_batch\":{},\"raw_limit_bytes_exclusive\":{},\"batch_deadline_seconds\":{},\"source_is_external_geometry\":false,\"line_coverage_rule\":\"UNKNOWN\",\"implementation_qualified\":false,\"projection_bits_are_native_readback\":false,\"families\":{},\"comparison_plan\":{}}}\n",
        s.cases.len(), 2 * s.cases.len(), extension::CASE_LIMIT, s.cases.len() * 8 * ffi::SLAB,
        extension::RAW_LIMIT_BYTES, extension::BATCH_SECONDS, array(groups), array(s.pairs.iter().map(pair_plan)))
}
pub fn diagnostic(
    p: &Pair,
    mode: usize,
    a: Option<&Vec<u8>>,
    b: Option<&Vec<u8>>,
    reference_clip_bits_equal: bool,
) -> String {
    let mut count = None;
    let mut lost = None;
    let mut gained = None;
    let mut xor = "null".to_string();
    if let Some((a, b)) = a.zip(b) {
        count = crate::check::differences(a, b);
        lost = Some(
            a.iter()
                .zip(b)
                .filter(|(a, b)| **a == 1 && **b == 0)
                .count(),
        );
        gained = Some(
            a.iter()
                .zip(b)
                .filter(|(a, b)| **a == 0 && **b == 1)
                .count(),
        );
        xor = mask_rows(&a.iter().zip(b).map(|(a, b)| a ^ b).collect::<Vec<_>>());
    }
    format!("{{\"schema\":\"generic-line-diagnostic-v2\",\"mode\":{mode},\"pair\":{},\"status\":{},\"differing_pixels\":{},\"lhs_only_pixels\":{},\"rhs_only_pixels\":{},\"xor_bitmap_rows\":{xor},\"CPU_reference_or_direct_input_clip_bits_equal\":{reference_clip_bits_equal},\"clip_bits_comparison_is_native_readback\":false}}\n",
        pair_plan(p), q(if count.is_some() { "OBSERVED" } else { "UNOBSERVED" }), option(count), option(lost), option(gained))
}
pub fn mask_rows(mask: &[u8]) -> String {
    array(mask.chunks_exact(ffi::SIDE).map(|row| {
        q(&row
            .iter()
            .map(|b| if *b == 0 { '0' } else { '1' })
            .collect::<String>())
    }))
}
pub fn assertion(a: &Assertion) -> String {
    format!("{{\"name\":{},\"passed\":{},\"lhs_case\":{},\"rhs_case\":{},\"differing_pixels\":{},\"expected_equal\":{}}}",
        q(&a.name), a.passed, option(a.lhs), option(a.rhs), option(a.differing_pixels), option(a.expected_equal))
}
pub fn option<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "null".into(), |v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_escaping() {
        assert_eq!(q("\"\\\n\u{1}"), "\"\\\"\\\\\\n\\u0001\"");
    }
    #[test]
    fn mask_rows_do_not_flip_or_transpose() {
        let mut m = vec![0; 1024];
        m[0] = 1;
        m[33] = 1;
        assert!(mask_rows(&m).starts_with("[\"10000000000000000000000000000000\",\"010"));
    }

    #[test]
    fn projection_receipts_never_claim_native_vertex_readback() {
        let s = extension::generate();
        for c in &s.cases {
            let text = projection_reference(&c.input);
            assert!(text.contains("\"native_vertex_readback\":false"));
            if c.input.draw_count != 0 {
                assert!(text.contains(if c.input.position_mode == 0 {
                    "CPU_staged_binary32_reference_NOT_native_vertex_bits"
                } else {
                    "direct_clip_input_identity_expression_not_vertex_readback"
                }));
            }
        }
    }
}
