#include <metal_stdlib>
using namespace metal;

// Fresh synthetic validation only. Inputs are eight raw binary32 words per row.
// All arithmetic below executes on the selected MTLDevice, never on the host.
constant uint poison = 0xa5a5a5a5u;

inline void header(device const uint *input, device uint *output, uint row, uint kind) {
    uint o = row * 32;
    for (uint k = 0; k < 32; ++k) output[o + k] = poison;
    output[o] = 0x51ad0000u | kind;
    output[o + 1] = row;
    for (uint k = 0; k < 8; ++k) output[o + 2 + k] = input[row * 8 + k];
}

inline float4 load4(device const uint *p) {
    return as_type<float4>(uint4(p[0], p[1], p[2], p[3]));
}

#define SIMPLE(NAME, KIND, EXPR) \
kernel void NAME(device const uint *input [[buffer(0)]], \
                 device uint *output [[buffer(1)]], \
                 constant uint &count [[buffer(2)]], uint row [[thread_position_in_grid]]) { \
    if (row >= count) return; \
    float4 a = load4(input + row * 8); \
    float4 b = load4(input + row * 8 + 4); \
    header(input, output, row, KIND); \
    output[row * 32 + 10] = as_type<uint>(EXPR); \
}

SIMPLE(dot2, 1, dot(a.xy, b.xy))
SIMPLE(dot3, 2, dot(a.xyz, b.xyz))
SIMPLE(dot4, 3, dot(a, b))
// Deliberately wrong arithmetic. Exact finite anchors distinguish it from dot.
SIMPLE(add2, 4, (a.x + b.x) + (a.y + b.y))
SIMPLE(add3, 5, ((a.x + b.x) + (a.y + b.y)) + (a.z + b.z))
SIMPLE(add4, 6, (((a.x + b.x) + (a.y + b.y)) + (a.z + b.z)) + (a.w + b.w))

kernel void noop(device const uint *input [[buffer(0)]],
                 device uint *output [[buffer(1)]],
                 constant uint &count [[buffer(2)]], uint row [[thread_position_in_grid]]) {
    // count is strictly positive in the host. A completed dispatch writes nothing.
    if (count == 0 && row == 0) output[0] = input[0];
}

kernel void compose_trace(device const uint *input [[buffer(0)]],
                          device uint *output [[buffer(1)]],
                          constant uint &count [[buffer(2)]], uint row [[thread_position_in_grid]]) {
    if (row >= count) return;
    float2 v = load4(input + row * 8).xy;
    float2 delta = load4(input + row * 8 + 4).xy;
    float length2 = dot(v, v);
    float inverse = fast::rsqrt(length2);
    float2 unit = v * inverse;
    float2 normal = float2(-unit.y, unit.x);
    // Keep negation and FMA operand order; finite algebra is not a NaN/zero rule.
    float projection = dot(-delta, normal);
    float magnitude = fabs(projection);
    bool inside = magnitude < 1.0f; // ordered source comparison, not isnan repair
    float2 candidate = fma(normal, float2(projection), delta);
    float2 chosen = select(delta, candidate, bool2(inside));
    header(input, output, row, 8);
    uint o = row * 32;
    output[o + 10] = as_type<uint>(length2);
    output[o + 11] = as_type<uint>(inverse);
    output[o + 12] = as_type<uint>(unit.x);
    output[o + 13] = as_type<uint>(unit.y);
    output[o + 14] = as_type<uint>(normal.x);
    output[o + 15] = as_type<uint>(normal.y);
    output[o + 16] = as_type<uint>(projection);
    output[o + 17] = as_type<uint>(magnitude);
    output[o + 18] = uint(inside);
    output[o + 19] = as_type<uint>(candidate.x);
    output[o + 20] = as_type<uint>(candidate.y);
    output[o + 21] = as_type<uint>(chosen.x);
    output[o + 22] = as_type<uint>(chosen.y);
    output[o + 23] = as_type<uint>(saturate(projection));
    output[o + 24] = as_type<uint>(saturate(fast::sqrt(projection)));
}

kernel void compose_sink(device const uint *input [[buffer(0)]],
                         device uint *output [[buffer(1)]],
                         constant uint &count [[buffer(2)]], uint row [[thread_position_in_grid]]) {
    if (row >= count) return;
    // Separate entry point: exposing intermediates can change optimization.
    float2 v = load4(input + row * 8).xy;
    float2 delta = load4(input + row * 8 + 4).xy;
    float2 unit = v * fast::rsqrt(dot(v, v));
    float2 normal = float2(-unit.y, unit.x);
    float projection = dot(-delta, normal);
    float magnitude = fabs(projection);
    bool inside = magnitude < 1.0f;
    float2 candidate = fma(normal, float2(projection), delta);
    float2 chosen = select(delta, candidate, bool2(inside));
    header(input, output, row, 9);
    uint o = row * 32;
    output[o + 21] = as_type<uint>(chosen.x);
    output[o + 22] = as_type<uint>(chosen.y);
    output[o + 23] = as_type<uint>(saturate(projection));
    output[o + 24] = as_type<uint>(saturate(fast::sqrt(projection)));
}
