#include <metal_stdlib>
using namespace metal;

// The wire format remains uint arrays: it never relies on host half3 padding.
struct Input {
    uint operation, tag, reserved0, reserved1;
    uint a[4], b[4], c[4];
};
struct Sample {
    uint operation, index, tag, stamp;
    uint a[4], b[4], c[4], result[4];
};

kernel void half_math_probe(device const Input *inputs [[buffer(0)]],
                            device Sample *samples [[buffer(1)]],
                            constant uint &count [[buffer(2)]],
                            uint index [[thread_position_in_grid]]) {
    if (index >= count) return;
    Input input = inputs[index];
    uint4 result = uint4(0u);
    half x = as_type<half>(ushort(input.a[0]));
    half y = as_type<half>(ushort(input.b[0]));
    switch (input.operation) {
        case 0:
            result.x = uint(as_type<ushort>(fmax3(x, y, as_type<half>(ushort(input.c[0])))));
            break;
        case 1:
            // Deliberately half arguments/result, not fast::powr(float, float).
            result.x = uint(as_type<ushort>(powr(x, y)));
            break;
        case 2: {
            half3 a = as_type<half3>(ushort3(input.a[0], input.a[1], input.a[2]));
            half3 b = as_type<half3>(ushort3(input.b[0], input.b[1], input.b[2]));
            result.xyz = uint3(as_type<ushort3>(fmin(a, b)));
            break;
        }
        case 3: {
            half3 a = as_type<half3>(ushort3(input.a[0], input.a[1], input.a[2]));
            half3 b = as_type<half3>(ushort3(input.b[0], input.b[1], input.b[2]));
            result.xyz = uint3(as_type<ushort3>(fmax(a, b)));
            break;
        }
        case 4: {
            half3 a = as_type<half3>(ushort3(input.a[0], input.a[1], input.a[2]));
            // Exact finite half-to-float conversion control, not a math/SFU oracle.
            result.xyz = as_type<uint3>(float3(a));
            break;
        }
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
