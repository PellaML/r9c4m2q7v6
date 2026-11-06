// Newly authored synthetic probe: RGBA32Float (16 B/pixel) compute store and
// direct read. Bits travel through as_type so no shader arithmetic touches them.
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

kernel void store_h4(texture2d<half, access::write> t [[texture(0)]],
                     const device ushort4 *in [[buffer(0)]],
                     constant uint2 &dims [[buffer(2)]],
                     uint2 g [[thread_position_in_grid]]) {
    if (g.x >= dims.x || g.y >= dims.y) return;
    t.write(as_type<half4>(in[g.y * dims.x + g.x]), g);
}

kernel void read_h4(texture2d<half, access::read> t [[texture(0)]],
                    device ushort4 *out [[buffer(1)]],
                    constant uint2 &dims [[buffer(2)]],
                    uint2 g [[thread_position_in_grid]]) {
    if (g.x >= dims.x || g.y >= dims.y) return;
    out[g.y * dims.x + g.x] = as_type<ushort4>(t.read(g));
}

kernel void sample_f4(texture2d<float> t [[texture(0)]],
                      sampler s [[sampler(0)]],
                      const device float2 *uv [[buffer(0)]],
                      device uint4 *out [[buffer(1)]],
                      constant uint2 &dims [[buffer(2)]],
                      uint2 g [[thread_position_in_grid]]) {
    if (g.x >= dims.x || g.y != 0) return;
    out[g.x] = as_type<uint4>(t.sample(s, uv[g.x], level(0.0)));
}