// Independently generated mathematical/native-API experiments only.
#include <metal_stdlib>
using namespace metal;

// Scalar stores avoid host/MSL struct-padding assumptions. Values are GPU bits.
inline void record4(device uint *out, uint i, uint tag, uint4 bits) {
    out[6u*i] = 0x51710000u ^ tag;
    out[6u*i+1u] = i;
    out[6u*i+2u] = bits.x; out[6u*i+3u] = bits.y;
    out[6u*i+4u] = bits.z; out[6u*i+5u] = bits.w;
}
kernel void read_float(texture2d<float, access::read> t [[texture(0)]],
                       device uint *out [[buffer(0)]],
                       constant uint4 &p [[buffer(1)]], uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    record4(out, xy.y*p.x+xy.x, p.z, as_type<uint4>(t.read(xy)));
}
kernel void read_half(texture2d<half, access::read> t [[texture(0)]],
                      device uint *out [[buffer(0)]],
                      constant uint4 &p [[buffer(1)]], uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    record4(out, xy.y*p.x+xy.x, p.z, uint4(as_type<ushort4>(t.read(xy))));
}
kernel void sample_float(texture2d<float, access::sample> t [[texture(0)]],
                         sampler s [[sampler(0)]], device uint *out [[buffer(0)]],
                         constant uint4 &p [[buffer(1)]], uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    // Off-center, away from nearest ties; last column exercises clamp-to-edge.
    float2 uv = (float2(xy) + float2(1.25f, 0.5f)) / float2(p.xy);
    record4(out, xy.y*p.x+xy.x, p.z, as_type<uint4>(t.sample(s, uv, level(0.0f))));
}
kernel void sample_half(texture2d<half, access::sample> t [[texture(0)]],
                        sampler s [[sampler(0)]], device uint *out [[buffer(0)]],
                        constant uint4 &p [[buffer(1)]], uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    float2 uv = (float2(xy) + float2(1.25f, 0.5f)) / float2(p.xy);
    record4(out, xy.y*p.x+xy.x, p.z, uint4(as_type<ushort4>(t.sample(s, uv, level(0.0f)))));
}
kernel void wrong_read_float(texture2d<float, access::read> t [[texture(0)]],
                             device uint *out [[buffer(0)]],
                             constant uint4 &p [[buffer(1)]], uint2 xy [[thread_position_in_grid]]) {
    if (xy.x >= p.x || xy.y >= p.y) return;
    record4(out, xy.y*p.x+xy.x, p.z, as_type<uint4>(t.read(xy).wzyx));
}

struct VertexOut { float4 position [[position]]; };
vertex VertexOut full_triangle(uint id [[vertex_id]]) {
    float2 p = float2(float((id << 1) & 2u), float(id & 2u));
    VertexOut out; out.position = float4(p*2.0f-1.0f, 0.0f, 1.0f); return out;
}
inline uint row_for(float4 pos) { return (uint(pos.y)-1u)*15u + uint(pos.x)-1u; }
inline void trace2(device uint4 *trace, uint i, uint tag, uint2 bits) {
    trace[i] = uint4(0x51720000u ^ tag, i, bits);
}
// No destination texture, framebuffer fetch, multiply, packed result, or B/A
// value exists in either positive fragment. Multiplication is fixed function.
fragment float2 output_float(VertexOut in [[stage_in]],
                              const device uint2 *inputs [[buffer(0)]],
                              device uint4 *trace [[buffer(1)]],
                              constant uint4 &p [[buffer(2)]]) {
    uint i = row_for(in.position);
    if (i >= p.x) { discard_fragment(); return float2(0.0f); }
    float2 value = as_type<float2>(inputs[i]);
    trace2(trace, i, p.y, as_type<uint2>(value)); return value;
}
fragment half2 output_half(VertexOut in [[stage_in]],
                            const device uint2 *inputs [[buffer(0)]],
                            device uint4 *trace [[buffer(1)]],
                            constant uint4 &p [[buffer(2)]]) {
    uint i = row_for(in.position);
    if (i >= p.x) { discard_fragment(); return half2(0.0h); }
    half2 value = half2(as_type<float2>(inputs[i]));
    trace2(trace, i, p.y, uint2(as_type<ushort2>(value))); return value;
}
// Genuine GPU negatives; neither the host nor validator edits GPU result bytes.
fragment float2 wrong_kernel(VertexOut in [[stage_in]],
                              const device uint2 *inputs [[buffer(0)]],
                              device uint4 *trace [[buffer(1)]],
                              constant uint4 &p [[buffer(2)]]) {
    uint i = row_for(in.position);
    if (i >= p.x) { discard_fragment(); return float2(0.0f); }
    float2 value = as_type<float2>(inputs[i]).yx;
    trace2(trace, i, p.y, as_type<uint2>(value)); return value;
}
fragment float2 wrong_output(VertexOut in [[stage_in]],
                              const device uint2 *inputs [[buffer(0)]],
                              device uint4 *trace [[buffer(1)]],
                              constant uint4 &p [[buffer(2)]]) {
    uint i = row_for(in.position);
    if (i >= p.x) { discard_fragment(); return float2(0.0f); }
    float2 intended = as_type<float2>(inputs[i]);
    trace2(trace, i, p.y, as_type<uint2>(intended)); return float2(0.0f);
}
