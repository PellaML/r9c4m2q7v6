#include <metal_stdlib>
using namespace metal;

struct Sample {
    uint word;
    uint index;
    ushort bits[4];
    uint stamp;
    uint reserved;
};

kernel void unpack_probe(device const uint *words [[buffer(0)]],
                         device Sample *samples [[buffer(1)]],
                         constant uint &count [[buffer(2)]],
                         uint index [[thread_position_in_grid]]) {
    if (index >= count) return;
    uint word = words[index];
    ushort4 bits = as_type<ushort4>(unpack_unorm4x8_to_half(word));
    samples[index].word = word;
    samples[index].index = index;
    for (uint lane = 0; lane < 4; ++lane) samples[index].bits[lane] = bits[lane];
    samples[index].stamp = word ^ index ^ 0x51c0ffeeu;
    samples[index].reserved = 0;
}
