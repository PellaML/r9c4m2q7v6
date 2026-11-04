#include <metal_stdlib>
using namespace metal;
#ifndef WRONG_EXPORT
#define WRONG_EXPORT 0
#endif
struct Varying { float4 position [[position]]; };
vertex Varying boundary_vertex(uint i [[vertex_id]]) {
    float2 p=float2(float((i<<1)&2u),float(i&2u));
    Varying v; v.position=float4(p*2.0f-1.0f,0.0f,1.0f);return v;
}
inline half4 audit_value(device const uint4 *input,device uint4 *echo,
                         device ushort4 *cast_bits,uint i) {
    uint4 bits=input[i];
    echo[i]=bits;
    half4 h=half4(as_type<float4>(bits));
    cast_bits[i]=as_type<ushort4>(h);
    return h;
}
kernel void boundary_audit(device const uint4 *input [[buffer(0)]],
        device uint4 *echo [[buffer(1)]],device ushort4 *cast_bits [[buffer(2)]],
        constant uint &count [[buffer(3)]],uint i [[thread_position_in_grid]]) {
    if(i<count) audit_value(input,echo,cast_bits,i);
}
fragment float4 boundary_float(Varying v [[stage_in]],
        device const uint4 *input [[buffer(0)]],device uint4 *echo [[buffer(1)]],
        device ushort4 *cast_bits [[buffer(2)]]) {
    uint i=uint(v.position.y)*8u+uint(v.position.x);
    audit_value(input,echo,cast_bits,i);
    float4 value=as_type<float4>(input[i]);
#if WRONG_EXPORT
    if(i==0u) value.r=-value.r; // Only exact anchor R changes; sidecars untouched.
#endif
    return value;
}
fragment half4 boundary_half(Varying v [[stage_in]],
        device const uint4 *input [[buffer(0)]],device uint4 *echo [[buffer(1)]],
        device ushort4 *cast_bits [[buffer(2)]]) {
    uint i=uint(v.position.y)*8u+uint(v.position.x);
    half4 value=audit_value(input,echo,cast_bits,i);
#if WRONG_EXPORT
    if(i==0u) value.r=-value.r;
#endif
    return value;
}
fragment float4 boundary_widen(Varying v [[stage_in]],
        device const uint4 *input [[buffer(0)]],device uint4 *echo [[buffer(1)]],
        device ushort4 *cast_bits [[buffer(2)]]) {
    uint i=uint(v.position.y)*8u+uint(v.position.x);
    half4 h=audit_value(input,echo,cast_bits,i);
    float4 value=float4(h);
#if WRONG_EXPORT
    if(i==0u) value.r=-value.r;
#endif
    return value;
}
