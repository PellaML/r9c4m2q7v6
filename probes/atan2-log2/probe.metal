#include <metal_stdlib>
using namespace metal;

// Generic two-argument record. Every operand is a runtime buffer value; no
// expectation, reference or constant result enters the kernel.
struct Input {
    uint a, b, tag, reserved;
};
struct Sample {
    uint a, b, index, stamp;
    uint result[4];
};

kernel void atan2_log2_probe(device const Input *inputs [[buffer(0)]],
                             device Sample *samples [[buffer(1)]],
                             constant uint &count [[buffer(2)]],
                             uint index [[thread_position_in_grid]]) {
    if (index >= count) return;
    Input input = inputs[index];
    float y = as_type<float>(input.a);
    float x = as_type<float>(input.b);
    samples[index].result[0] = as_type<uint>(fast::atan2(y, x));
    samples[index].result[1] = as_type<uint>(precise::atan2(y, x));
    samples[index].result[2] = as_type<uint>(fast::log2(y));
    samples[index].result[3] = as_type<uint>(precise::log2(y));
    samples[index].a = input.a;
    samples[index].b = input.b;
    samples[index].index = index;
    samples[index].stamp = 0x3d1f6a2bu ^ input.a ^ input.b ^ input.tag ^ index;
}
