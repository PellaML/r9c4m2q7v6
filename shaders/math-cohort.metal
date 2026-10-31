#include <metal_stdlib>
using namespace metal;

struct Input {
    uint operation, tag, reserved0, reserved1;
    uint a[4], b[4], c[4];
};
struct Sample {
    uint operation, index, tag, stamp;
    uint a[4], b[4], c[4], result[4];
};

// All arguments are runtime buffer values. No expected result enters this kernel.
kernel void math_probe(device const Input *inputs [[buffer(0)]],
                       device Sample *samples [[buffer(1)]],
                       constant uint &count [[buffer(2)]],
                       uint index [[thread_position_in_grid]]) {
    if (index >= count) return;
    Input input = inputs[index];
    uint4 result = uint4(0u);
    float x = as_type<float>(input.a[0]);
    float y = as_type<float>(input.b[0]);
    switch (input.operation) {
        case 0:
            result.x = as_type<uint>(fast::powr(x, y));
            break;
        case 1: {
            float4 a = as_type<float4>(uint4(input.a[0], input.a[1], input.a[2], input.a[3]));
            float4 b = as_type<float4>(uint4(input.b[0], input.b[1], input.b[2], input.b[3]));
            result = as_type<uint4>(fast::powr(a, b));
            break;
        }
        case 2:
            result.x = as_type<uint>(fast::exp(x));
            break;
        case 3:
            result.x = as_type<uint>(fast::log(x));
            break;
        case 4:
            // The compile profile selects the fast floating-point variant.
            result.x = as_type<uint>(fmax3(x, y, as_type<float>(input.c[0])));
            break;
        default:
            result = uint4(0xbad0c0deu);
            break;
    }
    uint stamp = 0x5ac3e271u ^ index ^ input.operation ^ input.tag;
    for (uint lane = 0; lane < 4; ++lane) {
        samples[index].a[lane] = input.a[lane];
        samples[index].b[lane] = input.b[lane];
        samples[index].c[lane] = input.c[lane];
        samples[index].result[lane] = result[lane];
        stamp ^= input.a[lane] ^ input.b[lane] ^ input.c[lane];
    }
    samples[index].operation = input.operation;
    samples[index].index = index;
    samples[index].tag = input.tag;
    samples[index].stamp = stamp;
}
