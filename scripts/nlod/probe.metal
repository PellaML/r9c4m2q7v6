// Newly authored synthetic probe: nearest/nearest, NotMipmapped, single-mip
// RGBA16F; two sampler states differing only in lodAverage. Each fragment
// samples one synthetic coordinate with five LOD methods and both samplers.
#include <metal_stdlib>
using namespace metal;

struct VOut { float4 position [[position]]; };

vertex VOut tri(uint vid [[vertex_id]]) {
    float2 p = float2((vid == 1) ? 3.0 : -1.0, (vid == 2) ? 3.0 : -1.0);
    VOut o; o.position = float4(p, 0.0, 1.0); return o;
}

static void five(texture2d<half> t, sampler s, float2 uv, device half4 *o) {
    o[0] = t.sample(s, uv);
    o[1] = t.sample(s, uv, level(0.0));
    o[2] = t.sample(s, uv, gradient2d(float2(0.001, 0.0), float2(0.0, 0.001)));
    o[3] = t.sample(s, uv, gradient2d(float2(4.0, 0.0), float2(0.0, 4.0)));
    o[4] = t.sample(s, uv, bias(4.0));
}

fragment half4 observe(VOut in [[stage_in]],
                       texture2d<half> tex [[texture(0)]],
                       sampler s0 [[sampler(0)]],
                       sampler s1 [[sampler(1)]],
                       const device float2 *uv [[buffer(0)]],
                       device half4 *out [[buffer(1)]],
                       constant uint2 &dims [[buffer(2)]]) {
    uint2 xy = uint2(in.position.xy);
    uint i = xy.y * dims.x + xy.x;
    if (i < dims.y) {
        float2 c = uv[i];
        five(tex, s0, c, out + i * 10);
        five(tex, s1, c, out + i * 10 + 5);
    }
    return half4(0.0h);
}