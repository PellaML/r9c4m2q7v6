// Independently owned geometry: one full-width, one-pixel-high quad per instance.
#include <metal_stdlib>
using namespace metal;
struct Params { uint height; uint wrap_value; };
struct Varying { float4 position [[position]]; float4 value [[flat]]; };
vertex Varying row_vertex(uint vertexID [[vertex_id]], uint instanceID [[instance_id]],
                         constant Params &p [[buffer(0)]]) {
    // Both strip orders and the indexed list use corners 0..3. Top is row zero.
    float x = (vertexID & 2u) ? 1.0f : -1.0f;
    float y = 1.0f - float(instanceID + (vertexID & 1u)) * (2.0f / float(p.height));
    // The negative wrap control changes ONLY values, not row placement.
    uint valueID = p.wrap_value ? (instanceID & 63u) : instanceID;
    float v = float(valueID + 1u);
    return {float4(x, y, 0.0f, 1.0f), float4(v, v / 8192.0f, v / 16384.0f, 0.5f)};
}
fragment float4 row_fragment(Varying in [[stage_in]]) { return in.value; }
