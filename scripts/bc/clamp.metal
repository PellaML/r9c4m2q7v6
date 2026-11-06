// Newly authored synthetic probe: float2 clamp variants. VARIANT selects one.
#include <metal_stdlib>
using namespace metal;
#ifndef VARIANT
#define VARIANT 0
#endif
kernel void observe(const device float2 *x [[buffer(0)]], const device float2 *lo [[buffer(1)]],
                    const device float2 *hi [[buffer(2)]], device float2 *out [[buffer(3)]],
                    constant uint &n [[buffer(4)]], uint i [[thread_position_in_grid]]) {
    if (i >= n) return;
    float2 a = x[i], l = lo[i], h = hi[i];
#if VARIANT == 0
    out[i] = clamp(a, l, h);
#elif VARIANT == 1
    out[i] = fast::clamp(a, l, h);
#elif VARIANT == 2
    out[i] = precise::clamp(a, l, h);
#elif VARIANT == 3
    out[i] = min(max(a, l), h);
#else
    out[i] = max(min(a, h), l);
#endif
}