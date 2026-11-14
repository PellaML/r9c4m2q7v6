// Synthetic inputs only. Blending is fixed function, not shader arithmetic.
#include <metal_stdlib>
using namespace metal;

struct Position { float4 position [[position]]; };
vertex Position triangle(uint id [[vertex_id]]) {
    float2 p = float2(float((id << 1) & 2u), float(id & 2u));
    Position out; out.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f);
    return out;
}

fragment half4 emit_f16(Position in [[stage_in]],
                        const device uint4 *input [[buffer(0)]],
                        device uint4 *trace [[buffer(1)]],
                        constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x) { discard_fragment(); return half4(0.0h); }
    uint4 original = input[row];
    half4 value = half4(as_type<float4>(original));
    trace[3u * row] = uint4(0x52474241u, row, 16u, p.y);
    trace[3u * row + 1u] = original;
    trace[3u * row + 2u] = uint4(as_type<ushort4>(value));
    return value;
}

fragment float4 emit_f32(Position in [[stage_in]],
                         const device uint4 *input [[buffer(0)]],
                         device uint4 *trace [[buffer(1)]],
                         constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x) { discard_fragment(); return float4(0.0f); }
    uint4 original = input[row];
    float4 value = as_type<float4>(original);
    trace[3u * row] = uint4(0x52474241u, row, 32u, p.y);
    trace[3u * row + 1u] = original;
    trace[3u * row + 2u] = as_type<uint4>(value);
    return value;
}
