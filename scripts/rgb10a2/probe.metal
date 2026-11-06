// Newly authored synthetic probe. Full-screen triangle; each fragment returns
// one synthetic float4 from a buffer indexed by its integer pixel position.
#include <metal_stdlib>
using namespace metal;

struct VOut { float4 position [[position]]; };

vertex VOut tri(uint vid [[vertex_id]]) {
    float2 p = float2((vid == 1) ? 3.0 : -1.0, (vid == 2) ? 3.0 : -1.0);
    VOut o; o.position = float4(p, 0.0, 1.0); return o;
}

fragment float4 store_float(VOut in [[stage_in]],
                            const device float4 *values [[buffer(0)]],
                            constant uint &width [[buffer(1)]]) {
    uint2 xy = uint2(in.position.xy);
    return values[xy.y * width + xy.x];
}

fragment half4 store_half(VOut in [[stage_in]],
                          const device float4 *values [[buffer(0)]],
                          constant uint &width [[buffer(1)]]) {
    uint2 xy = uint2(in.position.xy);
    return half4(values[xy.y * width + xy.x]);
}