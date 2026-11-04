#include <metal_stdlib>
using namespace metal;

#ifndef PROBE_SWAP_FRAGMENT
#define PROBE_SWAP_FRAGMENT 0
#endif
#ifndef PROBE_SWAP_READ
#define PROBE_SWAP_READ 0
#endif

// Generic RG16Float observation shaders. Every color operand is read from a
// runtime buffer; no value table lives in this source.
struct VertexOut {
    float4 position [[position]];
};

vertex VertexOut rg16_vertex(uint vid [[vertex_id]]) {
    // One triangle covering the whole viewport.
    float2 p = float2(float((vid << 1) & 2u), float(vid & 2u));
    VertexOut out;
    out.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f);
    return out;
}

fragment float4 rg16_fragment(VertexOut in [[stage_in]],
                              device const float4 *colors [[buffer(0)]],
                              constant uint &width [[buffer(1)]]) {
    uint x = uint(in.position.x);
    uint y = uint(in.position.y);
    float4 c = colors[y * width + x];
#if PROBE_SWAP_FRAGMENT
    // Deliberately wrong GPU program: the comparator MUST reject it.
    c = float4(c.y, c.x, c.z, c.w);
#endif
    return c;
}

kernel void rg16_read(texture2d<float, access::read> target [[texture(0)]],
                      texture2d<float, access::sample> sampled [[texture(1)]],
                      device float4 *out [[buffer(0)]],
                      uint2 gid [[thread_position_in_grid]]) {
    uint w = target.get_width();
    uint h = target.get_height();
    if (gid.x >= w || gid.y >= h) return;
    constexpr sampler nearest(coord::normalized, filter::nearest,
                              address::clamp_to_edge, mip_filter::none);
    float2 uv = (float2(gid) + 0.5f) / float2(w, h);
    float4 r = target.read(gid);
    float4 s = sampled.sample(nearest, uv, level(0.0f));
#if PROBE_SWAP_READ
    r = float4(r.y, r.x, r.z, r.w);
#endif
    uint i = gid.y * w + gid.x;
    out[2 * i] = r;
    out[2 * i + 1] = s;
}
#ifndef PROBE_WRONG_LOD
#define PROBE_WRONG_LOD 0
#endif

// Two real sampler states differ only in lodAverage. Upper rows exercise
// implicit fragment derivatives; lower rows use explicit, varying gradients.
fragment float4 rg16_lod_fragment(VertexOut in [[stage_in]],
        texture2d<float, access::sample> input [[texture(0)]],
        sampler individual [[sampler(0)]], sampler averaged [[sampler(1)]],
        device float4 *out [[buffer(0)]], constant uint &width [[buffer(1)]],
        device const float4 *params [[buffer(2)]]) {
    uint i = uint(in.position.y) * width + uint(in.position.x);
    float4 p = params[i];
    float4 a, b;
    if (uint(in.position.y) < 2u) {
        a = input.sample(individual, p.xy);
        b = input.sample(averaged, p.xy);
    } else {
        gradient2d g(float2(p.z, 0.0f), float2(0.0f, p.w));
        a = input.sample(individual, p.xy, g);
        b = input.sample(averaged, p.xy, g);
    }
#if PROBE_WRONG_LOD
    b.r += 1.0f; // Deliberately wrong result; states and texture remain valid.
#endif
    out[2*i] = a;
    out[2*i+1] = b;
    return a;
}