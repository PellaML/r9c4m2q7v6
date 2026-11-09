// Generic, buffer-driven signed16 observations. Host rejects all undefined inputs.
#include <metal_stdlib>
using namespace metal;

kernel void forward_s16(const device uint2 *input [[buffer(0)]],
                         device short2 *output [[buffer(1)]],
                         device uint4 *trace [[buffer(2)]],
                         constant uint4 &p [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.x) return;
    uint2 raw = input[i];
    short2 value = short2(as_type<float2>(raw));
    output[i] = value; // Actual i16x2 GPU storage, not a host narrowing.
    trace[i] = uint4(0x53160000u ^ p.y, i, raw);
}

// Separate entry point and later dispatch. The forward result is already stored
// and captured before this kernel reads that same buffer. No paired cancellation.
kernel void inverse_s16(const device short2 *input [[buffer(0)]],
                         device float2 *output [[buffer(1)]],
                         device uint4 *trace [[buffer(2)]],
                         constant uint4 &p [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.x) return;
    short2 value = input[i];
    output[i] = float2(value);
    trace[i] = uint4(0x53160000u ^ p.y, i, uint2(as_type<ushort2>(value)));
}

// Wrong rounding is dispatched ONLY for the first four small pairs. The host
// verifies abs(input)<=16 there; nearest rounding cannot leave signed16 range.
kernel void nearest_s16(const device uint2 *input [[buffer(0)]],
                         device short2 *output [[buffer(1)]],
                         device uint4 *trace [[buffer(2)]],
                         constant uint4 &p [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.x) return;
    uint2 raw = input[i];
    output[i] = short2(rint(as_type<float2>(raw)));
    trace[i] = uint4(0x53160000u ^ p.y, i, raw);
}

kernel void swap_s16(const device uint2 *input [[buffer(0)]],
                      device short2 *output [[buffer(1)]],
                      device uint4 *trace [[buffer(2)]],
                      constant uint4 &p [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.x) return;
    uint2 raw = input[i].yx;
    output[i] = short2(as_type<float2>(raw));
    trace[i] = uint4(0x53160000u ^ p.y, i, raw);
}
