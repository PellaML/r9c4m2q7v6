#include <metal_stdlib>
using namespace metal;

struct Input {
    uint f[2];
    ushort h[3];
    ushort reserved;
};
struct Sample {
    uint input_f[2];
    uint output_f[2];
    ushort input_h[3];
    ushort output_h[3];
    uint index;
    uint stamp;
    uint reserved;
};

kernel void fract_probe(device const Input *inputs [[buffer(0)]],
                        device Sample *samples [[buffer(1)]],
                        constant uint &count [[buffer(2)]],
                        uint index [[thread_position_in_grid]]) {
    if (index >= count) return;
    Input input = inputs[index];
    float2 f = as_type<float2>(uint2(input.f[0], input.f[1]));
    half3 h = as_type<half3>(ushort3(input.h[0], input.h[1], input.h[2]));
    uint2 fbits = as_type<uint2>(fract(f));
    ushort3 hbits = as_type<ushort3>(fract(h));
    for (uint lane = 0; lane < 2; ++lane) {
        samples[index].input_f[lane] = input.f[lane];
        samples[index].output_f[lane] = fbits[lane];
    }
    for (uint lane = 0; lane < 3; ++lane) {
        samples[index].input_h[lane] = input.h[lane];
        samples[index].output_h[lane] = hbits[lane];
    }
    samples[index].index = index;
    samples[index].stamp = input.f[0] ^ input.f[1]
        ^ (uint(input.h[0]) << 16) ^ uint(input.h[1]) ^ uint(input.h[2])
        ^ index ^ 0x91f6a37cu;
    samples[index].reserved = 0;
}
