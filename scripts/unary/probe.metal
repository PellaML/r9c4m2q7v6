#include <metal_stdlib>
using namespace metal;

#ifndef PROBE_VARIANT
#define PROBE_VARIANT 0
#endif
#if PROBE_VARIANT < 0 || PROBE_VARIANT > 2
#error unsupported_probe_variant
#endif

// Every operand comes from a runtime buffer, never a source literal/table.
// Packed uint words avoid float3/uint3 alignment and preserve all input bits.
kernel void unary_probe(device const uint *input [[buffer(0)]],
                        device uint *output [[buffer(1)]],
                        constant uint &count [[buffer(2)]],
                        uint i [[thread_position_in_grid]]) {
    if (i >= count) return;
    uint bits = input[i];
    float value = as_type<float>(bits);
    float sine = fast::sin(value);
    float cosine = fast::cos(value);
    output[3 * i] = bits;
#if PROBE_VARIANT == 1
    // Deliberately wrong GPU program: the anchor checker MUST reject it.
    output[3 * i + 1] = as_type<uint>(cosine);
    output[3 * i + 2] = as_type<uint>(sine);
#elif PROBE_VARIANT == 2
    output[3 * i + 1] = as_type<uint>(sine);
    output[3 * i + 2] = 0u;
#else
    output[3 * i + 1] = as_type<uint>(sine);
    output[3 * i + 2] = as_type<uint>(cosine);
#endif
}
