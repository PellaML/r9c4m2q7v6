// Synthetic public-API experiment. Color blending is fixed function, never MSL.
#include <metal_stdlib>
using namespace metal;

struct Position { float4 position [[position]]; };
vertex Position triangle(uint id [[vertex_id]]) {
    float2 p = float2(float((id << 1) & 2u), float(id & 2u));
    Position out; out.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f); return out;
}

// No user varyings or interpolated color: position only selects a synthetic row.
fragment float4 emit_f32(Position in [[stage_in]],
                         const device uint4 *input [[buffer(0)]],
                         device uint4 *trace [[buffer(1)]],
                         constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x || row == p.z) { discard_fragment(); return float4(0.0f); }
    float4 value = as_type<float4>(input[row]);
    trace[2u * row] = uint4(0x52313600u ^ p.y, row, 32u, 0x74726163u);
    trace[2u * row + 1u] = as_type<uint4>(value);
    return value;
}

fragment half4 emit_f16(Position in [[stage_in]],
                        const device uint4 *input [[buffer(0)]],
                        device uint4 *trace [[buffer(1)]],
                        constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x || row == p.z) { discard_fragment(); return half4(0.0h); }
    half4 value = half4(as_type<float4>(input[row]));
    trace[2u * row] = uint4(0x52313600u ^ p.y, row, 16u, 0x74726163u);
    trace[2u * row + 1u] = uint4(as_type<ushort4>(value));
    return value;
}

// Both typed reads use the SAME post-render R16Float texture, not a copied image.
// Their outputs are bit patterns, with no arithmetic on the loaded color.
kernel void observe(texture2d<float, access::read> f [[texture(0)]],
                    texture2d<half, access::read> h [[texture(1)]],
                    device uint4 *out [[buffer(0)]],
                    constant uint4 &p [[buffer(1)]],
                    uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) { return; }
    uint index = xy.y * p.x + xy.x;
    out[3u * index] = as_type<uint4>(f.read(xy));
    out[3u * index + 1u] = uint4(as_type<ushort4>(h.read(xy)));
    out[3u * index + 2u] = uint4(0x52454100u ^ p.z, xy, index);
}
