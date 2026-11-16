#include <metal_stdlib>
using namespace metal;

struct PositionOnly { float4 position [[position]]; };

// Only position is emitted. This is vertex transformation, NOT a coverage model.
vertex PositionOnly line_position(uint vid [[vertex_id]], uint iid [[instance_id]],
                            const device packed_float2 *points [[buffer(0)]],
                            constant float4 &viewport [[buffer(1)]],
                            constant packed_float2 *offsets [[buffer(2)]]) {
    float2 p = float2(points[vid]) + float2(offsets[iid]);
    float2 xy = ((p - viewport.xy) / viewport.zw) * float2(2.0f, -2.0f)
                + float2(-1.0f, 1.0f);
    return {float4(xy, 0.5f, 1.0f)};
}

// A full-screen triangle is used ONLY to query the stencil attachment.
vertex PositionOnly query_position(uint vid [[vertex_id]]) {
    const float2 p[3] = {float2(-1.0f, -1.0f), float2(3.0f, -1.0f),
                         float2(-1.0f, 3.0f)};
    return {float4(p[vid], 0.5f, 1.0f)};
}

fragment float4 query_white() { return float4(1.0f); }
