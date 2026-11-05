#include <metal_stdlib>
using namespace metal;
#ifndef WRONG_ENDPOINT
#define WRONG_ENDPOINT 0
#endif
struct Position { float4 position [[position]]; };
vertex Position clamp_vertex(uint i [[vertex_id]]) {
    float2 p=float2(float((i<<1)&2u),float(i&2u));
    Position v;v.position=float4(p*2.0f-1.0f,0.0f,1.0f);return v;
}
// Packed runtime records: a_bits,b_bits,lower_bits,upper_bits,route.
// route0 uses a directly; route1 performs genuine half a/b on the GPU.
fragment float4 clamp_fragment(Position p [[stage_in]],
        device const ushort *input [[buffer(0)]],device ushort *echo [[buffer(1)]],
        device ushort *intermediate [[buffer(2)]],device ushort *result [[buffer(3)]],
        constant uint &count [[buffer(4)]]) {
    uint i=uint(p.position.y)*8u+uint(p.position.x);
    if(i>=count)return float4(0.0f,0.0f,0.0f,1.0f);
    for(uint c=0;c<5u;c++)echo[5u*i+c]=input[5u*i+c];
    half a=as_type<half>(input[5u*i]);
    half b=as_type<half>(input[5u*i+1u]);
    half lo=as_type<half>(input[5u*i+2u]);
    half hi=as_type<half>(input[5u*i+3u]);
    half x;
    if(input[5u*i+4u]==0u)x=a;
    else x=a/b;
    // Both buffers contain GPU-produced raw half bits, with no color export
    // conversion or CPU pre-clamp in the numeric path.
    intermediate[i]=as_type<ushort>(x);
    half y=metal::clamp(x,lo,hi);
#if WRONG_ENDPOINT
    if(i==0u)y=lo; // Only the selected +Inf endpoint is deliberately wrong.
#endif
    result[2u*i]=as_type<ushort>(y);
    result[2u*i+1u]=ushort(0xd100u+i); // GPU completion marker for this result.
    return float4(0.0f,0.0f,0.0f,1.0f);
}
