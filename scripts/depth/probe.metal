#include <metal_stdlib>
using namespace metal;

#ifndef DEPTH_SHIFT
#define DEPTH_SHIFT 0
#endif
#if DEPTH_SHIFT != 0 && DEPTH_SHIFT != 1
#error DEPTH_SHIFT must be zero or one
#endif

// Layout constants only. All geometry, clip depths, w values, and tags arrive
// in a fresh host-generated buffer; there are no embedded vertex fixtures.
constant uint width = 8;
constant uint max_vertices = 12;
constant uint header_words = 16;
constant uint vertex_words = 16;
constant uint pixel_words = 16;
constant uint vertex_seen = 0x76580001u;

struct RasterVertex {
    float4 position [[position]];
    uint token [[flat]];
};

vertex RasterVertex depth_vertex(uint vid [[vertex_id]],
                                device const uint *input [[buffer(0)]],
                                device atomic_uint *audit [[buffer(1)]],
                                constant uint &count [[buffer(2)]]) {
    RasterVertex result;
    if (vid >= count || vid >= max_vertices) {
        atomic_fetch_add_explicit(audit + 0, 1u, memory_order_relaxed);
        result.position = float4(0.0f, 0.0f, 0.0f, 1.0f);
        result.token = 0;
        return result;
    }
    uint start = 8 * vid;
    uint4 bits(input[start], input[start + 1], input[start + 2], input[start + 3]);
    result.position = as_type<float4>(bits);
    result.token = input[start + 7];
#if DEPTH_SHIFT
    // Real negative: alter clip z before rasterization. The fragment shader
    // still records the actual position built-in, never a fabricated depth.
    result.position.z += result.position.w * 0.125f;
#endif
    uint base = header_words + vertex_words * vid;
    atomic_store_explicit(audit + base, vertex_seen, memory_order_relaxed);
    atomic_store_explicit(audit + base + 1, vid, memory_order_relaxed);
    for (uint i = 0; i != 8; ++i)
        atomic_store_explicit(audit + base + 2 + i, input[start + i], memory_order_relaxed);
    uint4 emitted = as_type<uint4>(result.position);
    for (uint i = 0; i != 4; ++i)
        atomic_store_explicit(audit + base + 10 + i, emitted[i], memory_order_relaxed);
    return result;
}

fragment float4 depth_fragment(RasterVertex in [[stage_in]],
                               uint primitive [[primitive_id]],
                               device atomic_uint *audit [[buffer(1)]]) {
    float4 p = in.position;
    // Unconditional derivatives precede every branch. Incomplete quads in the
    // partial-coverage case provide indirect (not direct-store) helper evidence.
    float4 derivatives(dfdx(p.z), dfdy(p.z), dfdx(p.w), dfdy(p.w));
    if (!all(isfinite(p.xy)) || any(p.xy < 0.0f) || any(p.xy >= float(width))) {
        atomic_fetch_add_explicit(audit + 1, 1u, memory_order_relaxed);
        return float4(0.0f);
    }
    uint2 xy = uint2(p.xy);
    uint base = header_words + max_vertices * vertex_words
              + pixel_words * (xy.y * width + xy.x);
    uint prior = atomic_fetch_add_explicit(audit + base, 1u, memory_order_relaxed);
    if (prior != 0)
        atomic_fetch_add_explicit(audit + 2, 1u, memory_order_relaxed);
    atomic_fetch_add_explicit(audit + 3, 1u, memory_order_relaxed);
    uint4 position_bits = as_type<uint4>(p);
    uint4 derivative_bits = as_type<uint4>(derivatives);
    for (uint i = 0; i != 4; ++i) {
        atomic_store_explicit(audit + base + 1 + i, position_bits[i], memory_order_relaxed);
        atomic_store_explicit(audit + base + 7 + i, derivative_bits[i], memory_order_relaxed);
    }
    atomic_store_explicit(audit + base + 5, primitive, memory_order_relaxed);
    atomic_store_explicit(audit + base + 6, in.token, memory_order_relaxed);
    // The color sink is discarded. No depth value travels through a color
    // attachment or a depth/stencil format; the evidence is raw float32 bits.
    return float4(0.0f);
}
