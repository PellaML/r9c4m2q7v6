#include <metal_stdlib>
using namespace metal;

// Standalone public experiment. Inputs are synthetic IEEE values supplied by
// the Rust host. No application shader, descriptor, or captured data is used.
#ifndef WRONG_VALUE
#define WRONG_VALUE 0
#endif

struct Raster { float4 position [[position]]; };
vertex Raster cover(uint id [[vertex_id]]) {
    const float2 positions[3] = {float2(-1, -1), float2(3, -1), float2(-1, 3)};
    Raster out;
    out.position = float4(positions[id], 0, 1);
    return out;
}

#if MULTI_TARGET
struct FloatColors { float4 first [[color(0)]]; float4 second [[color(1)]]; };
struct HalfColors { half4 first [[color(0)]]; half4 second [[color(1)]]; };
#else
typedef float4 FloatColors;
typedef half4 HalfColors;
#endif

#define ARGS Raster r [[stage_in]], device const uint4 *src [[buffer(0)]], \
    device const ushort4 *raw [[buffer(1)]], device uint4 *seen [[buffer(2)]], \
    device ushort4 *seen_half [[buffer(3)]], device uint4 *export_float [[buffer(4)]], \
    device ushort4 *export_half [[buffer(5)]], constant uint2 &shape [[buffer(6)]]
#define INDEX uint i = uint(r.position.y) * shape.x + uint(r.position.x)
#define ECHO seen[i] = src[i]; seen_half[i] = raw[i]

inline FloatColors colors(float4 v, uint i) {
    (void)i;
#if WRONG_VALUE
    if (i == 0u) v.r = -v.r; // Sidecars are deliberately left correct.
#endif
#if MULTI_TARGET
    return {v, v};
#else
    return v;
#endif
}
inline HalfColors colors(half4 v, uint i) {
    (void)i;
#if WRONG_VALUE
    if (i == 0u) v.r = -v.r;
#endif
#if MULTI_TARGET
    return {v, v};
#else
    return v;
#endif
}

fragment FloatColors emit_float(ARGS) {
    INDEX;
    ECHO;
    float4 v = as_type<float4>(src[i]);
    export_float[i] = as_type<uint4>(v);
    export_half[i] = ushort4(0xa55a); // Not a half conversion observation.
    return colors(v, i);
}
fragment HalfColors emit_half_words(ARGS) {
    INDEX;
    ECHO;
    half4 v = as_type<half4>(raw[i]);
    export_float[i] = as_type<uint4>(float4(v));
    export_half[i] = as_type<ushort4>(v);
    return colors(v, i);
}
fragment FloatColors emit_widened_words(ARGS) {
    INDEX;
    ECHO;
    half4 h = as_type<half4>(raw[i]);
    float4 v = float4(h);
    export_float[i] = as_type<uint4>(v);
    export_half[i] = as_type<ushort4>(h);
    return colors(v, i);
}
fragment HalfColors emit_cast_half(ARGS) {
    INDEX;
    ECHO;
    half4 h = half4(as_type<float4>(src[i]));
    export_float[i] = as_type<uint4>(float4(h));
    export_half[i] = as_type<ushort4>(h);
    return colors(h, i);
}
