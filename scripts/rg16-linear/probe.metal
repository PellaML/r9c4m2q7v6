// Owned public-API filtering experiment. No captured inputs or application code.
#include <metal_stdlib>
using namespace metal;

struct Position { float4 position [[position]]; };
vertex Position triangle(uint id [[vertex_id]]) {
    float2 p = float2(float((id << 1) & 2u), float(id & 2u));
    Position o; o.position = float4(p * 2.0f - 1.0f, 0.0f, 1.0f); return o;
}

// Runtime samplers; ordinary implicit-LOD fragment sampling, no bias/offset overload.
// Built-in position chooses one record. No varying or coordinate interpolation.
fragment float4 sample_f32(Position in [[stage_in]],
    texture2d<float> rg [[texture(0)]], texture2d<float> control [[texture(1)]], texture2d<float> red [[texture(2)]],
    sampler s [[sampler(0)]], const device uint4 *input [[buffer(0)]],
    device uint4 *trace [[buffer(1)]], constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x || row == p.z) { discard_fragment(); return float4(0.0f); }
    uint4 q = input[row]; float2 uv = as_type<float2>(q.xy);
    float4 a = rg.sample(s, uv);
    float4 b = control.sample(s, uv);
    float4 r = red.sample(s, uv);
    trace[5u * row] = uint4(0x52474600u ^ p.y, row, 32u, p.w);
    trace[5u * row + 1u] = q;
    trace[5u * row + 2u] = as_type<uint4>(a);
    trace[5u * row + 3u] = as_type<uint4>(b);
    trace[5u * row + 4u] = as_type<uint4>(r);
    return a;
}

fragment half4 sample_f16(Position in [[stage_in]],
    texture2d<half> rg [[texture(0)]], texture2d<half> control [[texture(1)]], texture2d<half> red [[texture(2)]],
    sampler s [[sampler(0)]], const device uint4 *input [[buffer(0)]],
    device uint4 *trace [[buffer(1)]], constant uint4 &p [[buffer(2)]]) {
    uint row = uint(in.position.y) - 1u;
    if (row >= p.x || row == p.z) { discard_fragment(); return half4(0.0h); }
    uint4 q = input[row]; float2 uv = as_type<float2>(q.xy);
    half4 a = rg.sample(s, uv);
    half4 b = control.sample(s, uv);
    half4 r = red.sample(s, uv);
    trace[5u * row] = uint4(0x52474600u ^ p.y, row, 16u, p.w);
    trace[5u * row + 1u] = q;
    trace[5u * row + 2u] = uint4(as_type<ushort4>(a));
    trace[5u * row + 3u] = uint4(as_type<ushort4>(b));
    trace[5u * row + 4u] = uint4(as_type<ushort4>(r));
    return a;
}

// Each pair of typed bindings references the SAME Private texture.
kernel void read_exact(texture2d<float, access::read> rf [[texture(0)]],
    texture2d<half, access::read> rh [[texture(1)]],
    texture2d<float, access::read> cf [[texture(2)]],
    texture2d<half, access::read> ch [[texture(3)]],
    texture2d<float, access::read> sf [[texture(4)]],
    texture2d<half, access::read> sh [[texture(5)]],
    device uint4 *out [[buffer(0)]], constant uint4 &p [[buffer(1)]],
    uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    uint i = xy.y * p.x + xy.x;
    out[8u*i] = uint4(0x52475200u ^ p.z, i, xy);
    out[8u*i+1u] = as_type<uint4>(rf.read(xy));
    out[8u*i+2u] = uint4(as_type<ushort4>(rh.read(xy)));
    out[8u*i+3u] = as_type<uint4>(cf.read(xy));
    out[8u*i+4u] = uint4(as_type<ushort4>(ch.read(xy)));
    out[8u*i+5u] = as_type<uint4>(sf.read(xy));
    out[8u*i+6u] = uint4(as_type<ushort4>(sh.read(xy)));
    out[8u*i+7u] = uint4(p.x, p.y, p.w, 0x72656164u);
}
