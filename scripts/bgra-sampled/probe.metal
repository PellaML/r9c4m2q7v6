// Owned synthetic shaders only. paint: same fragment pattern as the public BGRA render probe.
// fetch/probe: generic texture read() and sampler sample() of one BGRA8 view; no other inputs.
#include <metal_stdlib>
using namespace metal;

vertex float4 triangle(uint v [[vertex_id]]) {
    const float2 p[3] = {float2(-1, -1), float2(3, -1), float2(-1, 3)};
    return float4(p[v], 0, 1);
}

fragment float4 paint(float4 p [[position]], constant uint &c [[buffer(0)]]) {
    uint x = uint(p.x), y = uint(p.y);
    uint4 rgba((17*x + 3*y + 11*c + 1) & 255,
               (5*x + 29*y + 7*c + 37) & 255,
               (13*x + 7*y + 19*c + 89) & 255,
               33 + 63*((x + 3*y + c) & 3));
    return float4(rgba) / 255.0f;
}

kernel void fetch(texture2d<float, access::sample> t [[texture(0)]],
                  device float4 *out [[buffer(0)]],
                  uint2 g [[thread_position_in_grid]]) {
    uint w = t.get_width(), h = t.get_height();
    if (g.x >= w || g.y >= h) return;
    out[g.y * w + g.x] = t.read(g);
}

kernel void probe(texture2d<float, access::sample> t [[texture(0)]],
                  sampler s [[sampler(0)]],
                  const device float2 *uv [[buffer(0)]],
                  device float4 *out [[buffer(1)]],
                  constant uint &n [[buffer(2)]],
                  uint i [[thread_position_in_grid]]) {
    if (i >= n) return;
    out[i] = t.sample(s, uv[i], level(0));
}