// Synthetic public fixed-function experiment, adapted from scripts/rg16max.
// No MIN/MAX calculation, destination fetch, arithmetic packing, or CPU oracle.
#include <metal_stdlib>
using namespace metal;
struct VertexOut { float4 position [[position]]; };
struct Params { uint count; uint tag; uint skip; uint precision; };
struct Trace { uint4 header; uint4 bits; };
vertex VertexOut full_triangle(uint vid [[vertex_id]]) {
    float2 p = float2(float((vid << 1) & 2u), float(vid & 2u));
    VertexOut out;
    out.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f);
    return out;
}
fragment float4 probe_float(VertexOut in [[stage_in]],
                            const device uint4 *inputs [[buffer(0)]],
                            device Trace *trace [[buffer(1)]],
                            constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count || row == p.skip) { discard_fragment(); return float4(0.0f); }
    float4 value = as_type<float4>(inputs[row]);
    trace[row].header = uint4(0x6d696e00u ^ p.tag, row, p.precision, ~row);
    trace[row].bits = as_type<uint4>(value);
    return value;
}
fragment half4 probe_half(VertexOut in [[stage_in]],
                          const device ushort4 *inputs [[buffer(0)]],
                          device Trace *trace [[buffer(1)]],
                          constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count || row == p.skip) { discard_fragment(); return half4(0.0h); }
    half4 value = as_type<half4>(inputs[row]); // exact half operands, not float conversion
    trace[row].header = uint4(0x6d696e00u ^ p.tag, row, p.precision, ~row);
    trace[row].bits = uint4(as_type<ushort4>(value));
    return value;
}
// Independent entry points + untouched descriptor blend defaults are replacement
// controls. They measure float export separately from MIN's comparison order.
fragment float4 control_float(VertexOut in [[stage_in]],
                              const device uint4 *inputs [[buffer(0)]],
                              device Trace *trace [[buffer(1)]],
                              constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count) { discard_fragment(); return float4(0.0f); }
    float4 value = as_type<float4>(inputs[row]);
    trace[row].header = uint4(0x6d696e00u ^ p.tag, row, p.precision, ~row);
    trace[row].bits = as_type<uint4>(value);
    return value;
}
fragment half4 control_half(VertexOut in [[stage_in]],
                            const device ushort4 *inputs [[buffer(0)]],
                            device Trace *trace [[buffer(1)]],
                            constant Params &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.count) { discard_fragment(); return half4(0.0h); }
    half4 value = as_type<half4>(inputs[row]);
    trace[row].header = uint4(0x6d696e00u ^ p.tag, row, p.precision, ~row);
    trace[row].bits = uint4(as_type<ushort4>(value));
    return value;
}
