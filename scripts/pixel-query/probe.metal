// Synthetic single-level observations, not a filtering implementation.
#include <metal_stdlib>
using namespace metal;

struct Row {
    uint4 float_bits;
    ushort4 half_bits;
    uint2 reserved;
    uint4 queries;
    uint4 marker;
};
static_assert(sizeof(Row) == 64, "row ABI");
struct Position { float4 position [[position]]; };

Row pack(float4 f, half4 h, uint4 q, uint id, uint i, uint stage) {
    Row r;
    r.float_bits = as_type<uint4>(f);
    r.half_bits = as_type<ushort4>(h);
    r.reserved = uint2(0);
    r.queries = q;
    uint hash = 2166136261u;
    for (uint j = 0; j < 4; ++j) hash = (hash ^ r.float_bits[j]) * 16777619u;
    uint2 hh = as_type<uint2>(r.half_bits);
    for (uint j = 0; j < 2; ++j) hash = (hash ^ hh[j]) * 16777619u;
    for (uint j = 0; j < 2; ++j) hash = (hash ^ r.reserved[j]) * 16777619u;
    for (uint j = 0; j < 4; ++j) hash = (hash ^ q[j]) * 16777619u;
    r.marker = uint4(id, i, stage, hash);
    return r;
}

kernel void sample_explicit(texture2d<float> f [[texture(0)]],
                            texture2d<half> h [[texture(1)]], sampler s [[sampler(0)]],
                            const device uint4 *coords [[buffer(0)]],
                            device Row *out [[buffer(1)]],
                            constant uint4 &p [[buffer(2)]],
                            uint i [[thread_position_in_grid]]) {
    if (i >= 64u) return;
    float2 uv = as_type<float2>(p.y == 1u ? coords[i].zw : coords[i].xy);
    uint4 q(f.get_width(0u), f.get_height(0u), h.get_width(0u), h.get_height(0u));
    out[i] = pack(f.sample(s, uv, level(0.0f)), h.sample(s, uv, level(0.0f)), q, p.x, i, 0u);
}

vertex Position fullscreen(uint i [[vertex_id]]) {
    Position p;
    p.position = float4(i == 1u ? 3.0f : -1.0f, i == 2u ? 3.0f : -1.0f, 0.0f, 1.0f);
    return p;
}

fragment float4 sample_implicit(Position position [[stage_in]],
                                texture2d<float> f [[texture(0)]],
                                texture2d<half> h [[texture(1)]], sampler s [[sampler(0)]],
                                const device uint4 *coords [[buffer(0)]],
                                device Row *out [[buffer(1)]],
                                constant uint4 &p [[buffer(2)]]) {
    uint i = uint(position.position.y) * 8u + uint(position.position.x);
    if (i >= 64u) return float4(0.0f);
    float2 uv = as_type<float2>(p.y == 1u ? coords[i].zw : coords[i].xy);
    uint4 q(f.get_width(0u), f.get_height(0u), h.get_width(0u), h.get_height(0u));
    out[i] = pack(f.sample(s, uv), h.sample(s, uv), q, p.x, i, 1u);
    return float4(0.0f); // Dummy color; qualification reads integer buffer sidecars only.
}
