// Generic integer-colour/stencil probe. No shader depth or stencil outputs.
#include <metal_stdlib>
using namespace metal;

struct VertexOut { float4 position [[position]]; };
struct FragmentParams { uint4 color; uint4 flags; };

vertex VertexOut screen_triangle(const device uint4 *words [[buffer(0)]],
                                 uint vertexID [[vertex_id]]) {
    VertexOut v;
    v.position = as_type<float4>(words[vertexID]);
    return v;
}

// Deliberately no early_fragment_tests attribute and no fragment side effects.
fragment uint4 paint(VertexOut in [[stage_in]],
                     constant FragmentParams &p [[buffer(0)]]) {
    if (p.flags.x == 1u || (p.flags.x == 2u && in.position.x < float(p.flags.y) * 0.5f)) {
        discard_fragment();
    }
    return p.color;
}

// One draw for each reference 0..255, Equal, readMask=255, writeMask=0,
// Keep/Keep/Keep. The GPU stencil test, not a CPU simulation, selects a value.
fragment uint4 query_stencil(constant uint4 &p [[buffer(0)]]) {
    return uint4(p.x, 255u - p.x, 0x53u, 255u);
}

// Each phase writes its own row in a fresh poisoned, guarded shared buffer.
// p = (case tag, first phase, intentionally skipped phase, invocation count).
kernel void mark_phase(device uint4 *out [[buffer(0)]],
                       constant uint4 &p [[buffer(1)]],
                       uint i [[thread_position_in_grid]]) {
    if (i >= p.w) return;
    uint phase = p.y + i;
    if (phase >= 4u || phase == p.z) return;
    out[phase] = uint4(0x53543847u, phase, p.x, ~phase);
}
