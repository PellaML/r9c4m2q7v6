// Owned synthetic fragment output; no texture reads, samplers or compute stage.
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
