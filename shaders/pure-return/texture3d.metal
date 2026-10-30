#include <metal_stdlib>
using namespace metal;

struct ProbeInputs {
    float4 position [[position]];
    float2 uv;
};

constexpr sampler edge3(coord::normalized, address::clamp_to_edge, filter::nearest);
constexpr sampler zero3(coord::normalized, address::clamp_to_zero, filter::nearest);

fragment float4 v3_implicit_float(ProbeInputs in [[stage_in]],
                                texture3d<float> t [[texture(0)]],
                                sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f));
}

fragment float4 v3_bias_float(ProbeInputs in [[stage_in]],
                            texture3d<float> t [[texture(0)]],
                            sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f), bias(in.uv.x));
}

fragment float4 v3_level0_float(ProbeInputs in [[stage_in]],
                              texture3d<float> t [[texture(0)]],
                              sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f), level(0.0f));
}

fragment float4 v3_level0_edge_float(ProbeInputs in [[stage_in]],
                                   texture3d<float> t [[texture(0)]]) {
    return t.sample(edge3, float3(in.uv, 0.5f), level(0.0f));
}

fragment float4 v3_level0_zero_float(ProbeInputs in [[stage_in]],
                                   texture3d<float> t [[texture(0)]]) {
    return t.sample(zero3, float3(in.uv, 0.5f), level(0.0f));
}

fragment half4 v3_implicit_half(ProbeInputs in [[stage_in]],
                              texture3d<half> t [[texture(0)]],
                              sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f));
}

fragment half4 v3_bias_half(ProbeInputs in [[stage_in]],
                          texture3d<half> t [[texture(0)]],
                          sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f), bias(in.uv.x));
}

fragment half4 v3_level0_half(ProbeInputs in [[stage_in]],
                            texture3d<half> t [[texture(0)]],
                            sampler s [[sampler(0)]]) {
    return t.sample(s, float3(in.uv, 0.5f), level(0.0f));
}

fragment half4 v3_level0_edge_half(ProbeInputs in [[stage_in]],
                                 texture3d<half> t [[texture(0)]]) {
    return t.sample(edge3, float3(in.uv, 0.5f), level(0.0f));
}

fragment half4 v3_level0_zero_half(ProbeInputs in [[stage_in]],
                                 texture3d<half> t [[texture(0)]]) {
    return t.sample(zero3, float3(in.uv, 0.5f), level(0.0f));
}
