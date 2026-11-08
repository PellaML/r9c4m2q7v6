// Synthetic public-API render-target experiment. The blend is FIXED FUNCTION;
// these fragments never calculate MAX, access the destination, or pack a result.
#include <metal_stdlib>
using namespace metal;

struct VertexOut { float4 position [[position]]; };
struct Params { uint count; uint tag; uint skip; uint reserved; };

vertex VertexOut full_triangle(uint vid [[vertex_id]]) {
    float2 p = float2(float((vid << 1) & 2u), float(vid & 2u));
    VertexOut out;
    out.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f);
    return out;
}

fragment float2 probe_float(VertexOut in [[stage_in]],
                            const device uint2 *inputs [[buffer(0)]],
                            device uint4 *trace [[buffer(1)]],
                            constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count || row == p.skip) { discard_fragment(); return float2(0.0f); }
    float2 value = as_type<float2>(inputs[row]);
    trace[row] = uint4(0x6d617800u ^ p.tag, row, as_type<uint2>(value));
    return value; // native float2 color output, not a half conversion
}

fragment half2 probe_half(VertexOut in [[stage_in]],
                          const device uint2 *inputs [[buffer(0)]],
                          device uint4 *trace [[buffer(1)]],
                          constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count || row == p.skip) { discard_fragment(); return half2(0.0h); }
    half2 value = half2(as_type<float2>(inputs[row]));
    trace[row] = uint4(0x6d617800u ^ p.tag, row, uint2(as_type<ushort2>(value)));
    return value; // native half2 output; its actual pre-export half bits are logged
}

// Separate entry points and untouched blend defaults provide an independent
// GPU replacement control. The host allocates a fresh target for every capture.
fragment float2 control_float(VertexOut in [[stage_in]],
                              const device uint2 *inputs [[buffer(0)]],
                              device uint4 *trace [[buffer(1)]],
                              constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count) { discard_fragment(); return float2(0.0f); }
    float2 value = as_type<float2>(inputs[row]);
    trace[row] = uint4(0x6d617800u ^ p.tag, row, as_type<uint2>(value));
    return value;
}

fragment half2 control_half(VertexOut in [[stage_in]],
                            const device uint2 *inputs [[buffer(0)]],
                            device uint4 *trace [[buffer(1)]],
                            constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count) { discard_fragment(); return half2(0.0h); }
    half2 value = half2(as_type<float2>(inputs[row]));
    trace[row] = uint4(0x6d617800u ^ p.tag, row, uint2(as_type<ushort2>(value)));
    return value;
}
