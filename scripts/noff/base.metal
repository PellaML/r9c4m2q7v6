// Newly authored synthetic RGBA32Float offset observation.
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

// Baseline with no offset, using the same coordinates as the offset cohort.
kernel void sample_plain(texture2d<float> t [[texture(0)]],
                         sampler s [[sampler(0)]],
                         const device float2 *uv [[buffer(0)]],
                         device uint4 *out [[buffer(1)]],
                         constant uint2 &dims [[buffer(2)]],
                         uint g [[thread_position_in_grid]]) {
    if (g >= dims.x) return;
    out[g] = as_type<uint4>(t.sample(s, uv[g]));
}
