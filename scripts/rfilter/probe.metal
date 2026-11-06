#include <metal_stdlib>
using namespace metal;
// Synthetic generic filtering observation. Same normalized UVs, same sampler, three formats.
constexpr sampler s(coord::normalized, address::clamp_to_edge, filter::linear, mip_filter::none);
kernel void observe(texture2d<half, access::sample> r1 [[texture(0)]],
                    texture2d<half, access::sample> r2 [[texture(1)]],
                    texture2d<half, access::sample> r4 [[texture(2)]],
                    device const float2 *uv [[buffer(0)]],
                    device ushort4 *o1 [[buffer(1)]],
                    device ushort4 *o2 [[buffer(2)]],
                    device ushort4 *o4 [[buffer(3)]],
                    constant uint &n [[buffer(4)]],
                    uint i [[thread_position_in_grid]]) {
    if (i >= n) return;
    float2 c = uv[i];
    o1[i] = as_type<ushort4>(r1.sample(s, c, level(0)));
    o2[i] = as_type<ushort4>(r2.sample(s, c, level(0)));
    o4[i] = as_type<ushort4>(r4.sample(s, c, level(0)));
}
