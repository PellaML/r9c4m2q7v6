#include <metal_stdlib>
using namespace metal;

// Supplied binary32 values only: no arithmetic, sampling, reduction or half cast.
kernel void rounding_write(device const float4 *values [[buffer(0)]],
                           device uint4 *echo [[buffer(1)]],
                           texture2d<float, access::write> output [[texture(0)]],
                           uint x [[thread_position_in_grid]]) {
    if (x >= output.get_width()) return;
    float4 value = values[x];
    output.write(value, uint2(x, 0));
    echo[x] = as_type<uint4>(value);
}
