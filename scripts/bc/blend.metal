// Newly authored synthetic probe: rgb10a2Unorm premultiplied source-over.
#include <metal_stdlib>
using namespace metal;
struct VOut { float4 position [[position]]; };
vertex VOut tri(uint vid [[vertex_id]]) {
    float2 p = float2((vid == 1) ? 3.0 : -1.0, (vid == 2) ? 3.0 : -1.0);
    VOut o; o.position = float4(p, 0.0, 1.0); return o;
}
static uint at(VOut in, uint width) { uint2 xy = uint2(in.position.xy); return xy.y * width + xy.x; }
fragment float4 store_float(VOut in [[stage_in]], const device float4 *v [[buffer(0)]], constant uint &w [[buffer(1)]]) {
    return v[at(in, w)];
}
fragment half4 store_half(VOut in [[stage_in]], const device float4 *v [[buffer(0)]], constant uint &w [[buffer(1)]]) {
    return half4(v[at(in, w)]);
}