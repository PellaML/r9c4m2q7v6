// Generic runtime half inputs, raw sidecar observations. No CPU-selected endpoint.
// Adapted from the existing public scalar clamp fragment and guarded trace probes.
#include <metal_stdlib>
using namespace metal;
struct Position { float4 position [[position]]; };
struct Input { ushort a, b, lower, upper; uint route, bound; };
struct Trace { uint2 header; ushort4 operands; ushort4 values; ushort4 inverse; };
struct Params { uint count, tag, skip, width; };
static_assert(sizeof(half) == 2 && sizeof(Input) == 16 && sizeof(Trace) == 32 && sizeof(Params) == 16,
              "public raw-half record layout");
vertex Position full_triangle(uint i [[vertex_id]]) {
    float2 p = float2(float((i << 1) & 2u), float(i & 2u));
    Position v; v.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f); return v;
}
fragment float4 observe_half(Position p [[stage_in]],
        const device Input *input [[buffer(0)]], device Trace *trace [[buffer(1)]],
        constant Params &params [[buffer(2)]]) {
    uint i = uint(p.position.y) * params.width + uint(p.position.x);
    if (i >= params.count || i == params.skip) { return float4(0.0f); }
    Input c = input[i];
    half a = as_type<half>(c.a), b = as_type<half>(c.b);
    half lower = as_type<half>(c.lower), upper = as_type<half>(c.upper);
    // route 1 is only an owned synthetic composition control. Its actual
    // intermediate is observed; no CPU assertion that fast division makes NaN.
    half x = c.route == 0u ? a : half(a / b);
    half dynamic_clamp = metal::clamp(x, lower, upper);
    half literal_clamp = metal::clamp(x, half(0.0h), half(1.0h));
    half saturate_control = metal::saturate(x); // separate operator, never the oracle for clamp
    trace[i].header = uint2(0x68636c00u ^ (params.tag << 16) ^ (c.route << 8) ^ c.bound, i);
    trace[i].operands = ushort4(c.a, c.b, c.lower, c.upper);
    ushort4 observed = ushort4(as_type<ushort>(x), as_type<ushort>(dynamic_clamp),
                              as_type<ushort>(literal_clamp), as_type<ushort>(saturate_control));
    trace[i].values = observed;
    // A missing numeric store cannot masquerade as an observed NaN endpoint:
    // value and bitwise inverse cannot both equal the initialized poison word.
    trace[i].inverse = observed ^ ushort4(0xffffu);
    // Dummy color output only. Numeric results are untouched raw half sidecars,
    // not render-target export or blending results.
    return float4(0.0f, 0.0f, 0.0f, 1.0f);
}
