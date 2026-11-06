// Newly authored synthetic probe (base library): RGBA32Float compute store,
// direct read, normalized clamp-to-edge nearest comparison sampling, and a
// guessed two-pass integral image (row pass via read, column-pass via read).
#include <metal_stdlib>
using namespace metal;

kernel void store_f4(texture2d<float, access::write> t [[texture(0)]],
                     const device uint4 *in [[buffer(0)]],
                     constant uint2 &dims [[buffer(2)]],
                     uint2 g [[thread_position_in_grid]]) {
    if (g.x >= dims.x || g.y >= dims.y) return;
    t.write(as_type<float4>(in[g.y * dims.x + g.x]), g);
}

kernel void read_f4(texture2d<float, access::read> t [[texture(0)]],
                    device uint4 *out [[buffer(1)]],
                    constant uint2 &dims [[buffer(2)]],
                    uint2 g [[thread_position_in_grid]]) {
    if (g.x >= dims.x || g.y >= dims.y) return;
    out[g.y * dims.x + g.x] = as_type<uint4>(t.read(g));
}

kernel void sample_norm_level0(texture2d<float> t [[texture(0)]],
                               sampler s [[sampler(0)]],
                               const device float2 *uv [[buffer(0)]],
                               device uint4 *out [[buffer(1)]],
                               constant uint2 &dims [[buffer(2)]],
                               uint g [[thread_position_in_grid]]) {
    if (g >= dims.x) return;
    out[g] = as_type<uint4>(t.sample(s, uv[g], level(0.0)));
}

// pass 1: one thread per row, sequential f32 adds left to right.
kernel void integral_rows(texture2d<float, access::read> src [[texture(0)]],
                          texture2d<float, access::write> dst [[texture(1)]],
                          constant uint2 &dims [[buffer(2)]],
                          uint g [[thread_position_in_grid]]) {
    if (g >= dims.y) return;
    float4 acc = float4(0.0f);
    for (uint x = 0; x < dims.x; ++x) { acc += src.read(uint2(x, g)); dst.write(acc, uint2(x, g)); }
}

// pass 2 reference: one thread per column, direct read, sequential adds top to bottom.
kernel void integral_cols_read(texture2d<float, access::read> src [[texture(0)]],
                               texture2d<float, access::write> dst [[texture(1)]],
                               constant uint2 &dims [[buffer(2)]],
                               uint g [[thread_position_in_grid]]) {
    if (g >= dims.x) return;
    float4 acc = float4(0.0f);
    for (uint y = 0; y < dims.y; ++y) { acc += src.read(uint2(g, y)); dst.write(acc, uint2(g, y)); }
}