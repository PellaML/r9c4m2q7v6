// Generic public Metal sampling experiment. Every offset is a source literal.
#include <metal_stdlib>
using namespace metal;

struct Input {
    uint2 normalized_bits;
    uint2 pixel_bits;
    uint offset_case;
    uint tag;
    uint2 reserved;
};
struct Params { uint count, normalized, run, reserved; };
struct Output { uint stamp, tag, shape, mips; uint4 value; };

template<typename T>
vec<T, 4> sample_literal(texture2d<T> image, sampler state, float2 uv, uint selection) {
    switch (selection) {
        case 0:  return image.sample(state, uv, level(0.0f), int2(-1, 0));
        case 1:  return image.sample(state, uv, level(0.0f), int2(-1, 1));
        case 2:  return image.sample(state, uv, level(0.0f), int2(0, -1));
        case 3:  return image.sample(state, uv, level(0.0f), int2(0, 0));
        case 4:  return image.sample(state, uv, level(0.0f), int2(0, 1));
        case 5:  return image.sample(state, uv, level(0.0f), int2(0, 2));
        case 6:  return image.sample(state, uv, level(0.0f), int2(1, -1));
        case 7:  return image.sample(state, uv, level(0.0f), int2(1, 0));
        case 8:  return image.sample(state, uv, level(0.0f), int2(1, 1));
        case 9:  return image.sample(state, uv, level(0.0f), int2(1, 2));
        case 10: return image.sample(state, uv, level(0.0f), int2(2, 0));
        case 11: return image.sample(state, uv, level(0.0f), int2(2, 1));
        case 12: return image.sample(state, uv, level(0.0f), int2(-8, -8));
        case 13: return image.sample(state, uv, level(0.0f), int2(7, 7));
        case 14: return image.sample(state, uv, level(0.0f), int2(-8, 7));
        case 15: return image.sample(state, uv, level(0.0f), int2(7, -8));
        default: return vec<T, 4>(T(0));
    }
}

kernel void sample_offset_float(
    device const Input *inputs [[buffer(0)]], device Output *outputs [[buffer(1)]],
    constant Params &p [[buffer(2)]], texture2d<float> image [[texture(0)]],
    sampler state [[sampler(0)]], uint index [[thread_position_in_grid]]) {
    if (index >= p.count) return;
    Input i = inputs[index];
    float2 uv = as_type<float2>(p.normalized ? i.normalized_bits : i.pixel_bits);
    float4 color = sample_literal(image, state, uv, i.offset_case);
    outputs[index] = {0x53504c00u ^ p.run, i.tag,
        image.get_width() | (image.get_height() << 16), image.get_num_mip_levels(),
        as_type<uint4>(color)};
}

kernel void sample_offset_half(
    device const Input *inputs [[buffer(0)]], device Output *outputs [[buffer(1)]],
    constant Params &p [[buffer(2)]], texture2d<half> image [[texture(0)]],
    sampler state [[sampler(0)]], uint index [[thread_position_in_grid]]) {
    if (index >= p.count) return;
    Input i = inputs[index];
    float2 uv = as_type<float2>(p.normalized ? i.normalized_bits : i.pixel_bits);
    half4 color = sample_literal(image, state, uv, i.offset_case);
    outputs[index] = {0x53504c00u ^ p.run, i.tag,
        image.get_width() | (image.get_height() << 16), image.get_num_mip_levels(),
        as_type<uint4>(float4(color))};
}
