// Newly authored synthetic probe (px_nolod): sampler74-style pixel-coordinate
// nearest sampling of RGBA32Float, isolated in its own library so a compile
// failure of this LOD spelling cannot hide the other results.
#include <metal_stdlib>
using namespace metal;

kernel void sample_px(texture2d<float> t [[texture(0)]],
                      sampler s [[sampler(0)]],
                      const device float2 *uv [[buffer(0)]],
                      device uint4 *out [[buffer(1)]],
                      constant uint2 &dims [[buffer(2)]],
                      uint g [[thread_position_in_grid]]) {
    if (g >= dims.x) return;
    out[g] = as_type<uint4>(t.sample(s, uv[g]));
}

// pass 2 as guessed for the second pass: column prefix sum whose loads go
// through the sampler at pixel centers.
kernel void integral_cols_px(texture2d<float> src [[texture(0)]],
                             texture2d<float, access::write> dst [[texture(1)]],
                             sampler s [[sampler(0)]],
                             constant uint2 &dims [[buffer(2)]],
                             uint g [[thread_position_in_grid]]) {
    if (g >= dims.x) return;
    float4 acc = float4(0.0f);
    for (uint y = 0; y < dims.y; ++y) {
        acc += src.sample(s, float2(float(g) + 0.5f, float(y) + 0.5f));
        dst.write(acc, uint2(g, y));
    }
}