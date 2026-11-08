// Public, synthetic geometry only. No orientation calculation in either shader.
#include <metal_stdlib>
using namespace metal;
struct VertexOut { float4 position [[position]]; };

vertex VertexOut vertex_bits(uint id [[vertex_id]],
                            const device uint4 *vertices [[buffer(0)]]) {
    VertexOut out;
    out.position = as_type<float4>(vertices[id]);
    return out;
}

fragment uint4 facing_bit(VertexOut in [[stage_in]],
                          bool front [[front_facing]],
                          constant uint2 &params [[buffer(0)]]) {
    uint2 pixel = uint2(in.position.xy);
    // Actual GPU negative: preserve the seed row via fragment discard.
    if (pixel.y == params.y) { discard_fragment(); return uint4(0u); }
    return uint4(uint(front), params.x, pixel.x, pixel.y);
}
