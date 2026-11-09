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
    if (row >= p.x || row == p.z || ((p.y & 256u) != 0u && row >= 32u && row < 38u && row != 34u)) { discard_fragment(); return float4(0.0f); }
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
    if (row >= p.x || row == p.z || ((p.y & 256u) != 0u && row >= 32u && row < 38u && row != 34u)) { discard_fragment(); return half4(0.0h); }
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

// Diagnostic arithmetic controls ONLY, never the render path or an oracle.
// Compile the SAME function precise/fast and retain its exact linked AIR.
// "plain" describes the source expression: inspect AIR to learn contraction.
kernel void arithmetic(const device uint4 *source [[buffer(0)]],
                       const device ushort *seed [[buffer(1)]],
                       device uint4 *out [[buffer(2)]],
                       constant uint4 &p [[buffer(3)]],
                       uint row [[thread_position_in_grid]]) {
    if (row >= p.x || ((p.z & 256u) != 0u && row >= 32u && row < 38u && row != 34u)) { return; }
    float original = as_type<float>(source[row].x);
    half hs = half(original);
    float s = p.y == 16u ? float(hs) : original;
    ushort dbits = seed[(row + 1u) * 128u + 1u];
    float d = float(as_type<half>(dbits));
    float factor = 1.0f - s;
    float product = d * factor;
    float plain = s + product;
    float outer = fma(d, factor, s);
    float expanded = (s + d) - (s * d);
    float expanded_fma = fma(-d, s, s + d);
    half hd = as_type<half>(dbits);
    half hk = half(1.0h - hs);
    half hp = half(hd * hk);
    half hplain = hs + hp;
    half hfused = fma(hd, hk, hs);
    out[6u * row] = uint4(0x554e4900u ^ p.z, row, p.y, as_type<uint>(s));
    out[6u * row + 1u] = uint4(uint(dbits), as_type<uint>(d), as_type<uint>(factor), as_type<uint>(product));
    out[6u * row + 2u] = as_type<uint4>(float4(plain, outer, expanded, expanded_fma));
    out[6u * row + 3u] = uint4(uint(as_type<ushort>(hs)), uint(as_type<ushort>(half(plain))),
                              uint(as_type<ushort>(half(outer))), source[row].x);
    out[6u * row + 4u] = uint4(uint(as_type<ushort>(hk)), uint(as_type<ushort>(hp)),
                              uint(as_type<ushort>(hplain)), uint(as_type<ushort>(hfused)));
    out[6u * row + 5u] = uint4(as_type<uint>(float(hplain)), as_type<uint>(float(hfused)), 0x68616c66u, uint(dbits));
}
