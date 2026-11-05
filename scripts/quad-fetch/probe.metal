#include <metal_stdlib>
using namespace metal;
#ifndef FAULT
#define FAULT 0
#endif

// Synthetic public MRT experiment: a real attachment color input, never a
// sampled texture standing in for framebuffer fetch. No application source.
struct Params { uint width, height, mode, seed, unused, variant, pad0, pad1; };
struct Raster { float4 position [[position]]; float2 value; };
struct Colors { half4 diagnostic [[color(0)]]; half2 attachment [[color(1)]]; };
struct Trace { uint word[32]; };

vertex Raster vertex_plane(uint id [[vertex_id]], constant float2 *points [[buffer(0)]],
                           constant Params &p [[buffer(1)]]) {
    float2 s = points[id];
    Raster r;
    r.position = float4(2.0f*s.x/float(p.width)-1.0f, 1.0f-2.0f*s.y/float(p.height), 0, 1);
    r.value = float2(3.0f*s.x+5.0f*s.y+1.0f, 2.0f*s.x-4.0f*s.y+7.0f);
    return r;
}
inline half2 analytic(float2 p, uint seed) {
    if (seed == 1u) return half2(7.0h, -5.0h);
    return half2(2.0f*p.x+4.0f*p.y+5.0f, 3.0f*p.x-2.0f*p.y+31.5f);
}
fragment Colors seed_plane(Raster r [[stage_in]], constant Params &p [[buffer(1)]]) {
    return {half4(-101.0h,-102.0h,-103.0h,-104.0h), analytic(r.position.xy,p.seed)};
}
inline uint pack(ushort2 h) { return uint(h.x) | (uint(h.y)<<16); }
inline Colors record(Raster r, half2 prior, float4 pos_d, float4 vary_d, float4 operand_d,
                     ushort2 hx, ushort2 hy, half4 color, device Trace *traces, constant Params &p) {
    uint i = uint(r.position.y)*p.width+uint(r.position.x);
    Trace t = {};
    t.word[0]=0x13570000u | (i+1u); t.word[1]=p.mode;
    uint2 xy=as_type<uint2>(r.position.xy), uv=as_type<uint2>(r.value);
    t.word[2]=xy.x; t.word[3]=xy.y; t.word[4]=uv.x; t.word[5]=uv.y;
    t.word[6]=pack(as_type<ushort2>(prior)); t.word[7]=p.seed;
    uint4 pd=as_type<uint4>(pos_d), vd=as_type<uint4>(vary_d), od=as_type<uint4>(operand_d);
    for (uint c=0;c<4;c++) { t.word[8+c]=pd[c];t.word[12+c]=vd[c];t.word[16+c]=od[c]; }
    t.word[20]=pack(hx);t.word[21]=pack(hy);
    ushort4 out=as_type<ushort4>(color);
    t.word[22]=pack(out.xy);t.word[23]=pack(out.zw);t.word[24]=pack(as_type<ushort2>(prior));
    t.word[25]=p.variant;
    traces[i]=t; // Helper stores have no effect; only active stores are evidence.
#if FAULT == 1
    if (i==0u) color.x += 1.0h; // Deliberately wrong output, trace left intact.
#endif
    return {color,prior};
}
inline void independent(Raster r, thread float4 &pd, thread float4 &vd) {
    pd=float4(dfdx(r.position.xy),dfdy(r.position.xy));
    vd=float4(dfdx(r.value),dfdy(r.value));
#if FAULT == 2
    vd.x += 1.0f; // Deliberately wrong derivative check control.
#endif
}
#define INPUTS Raster r [[stage_in]], half2 prior [[color(1)]], \
    device Trace *traces [[buffer(0)]], constant Params &p [[buffer(1)]]

// This entry has no executed or referenced derivative operation.
fragment Colors fetch_only(INPUTS) {
    return record(r,prior,float4(0),float4(0),float4(0),ushort2(0),ushort2(0),
                  half4(prior,19.0h,23.0h),traces,p);
}
fragment Colors fetch_independent(INPUTS) {
    float4 pd,vd;independent(r,pd,vd);
    half4 color=half4(float2(prior)+float2(pd.x,pd.w),vd.x,vd.z);
    return record(r,prior,pd,vd,float4(0),ushort2(0),ushort2(0),color,traces,p);
}
fragment Colors fetch_half_derivative(INPUTS) {
    float4 pd,vd;independent(r,pd,vd);
    half2 x=dfdx(prior),y=dfdy(prior);
    float4 d=float4(float2(x),float2(y));
    return record(r,prior,pd,vd,d,as_type<ushort2>(x),as_type<ushort2>(y),half4(x,y),traces,p);
}
fragment Colors fetch_float_derivative(INPUTS) {
    float4 pd,vd;independent(r,pd,vd);
    float2 v=float2(prior),x=dfdx(v),y=dfdy(v);
    float4 d=float4(x,y);
    return record(r,prior,pd,vd,d,ushort2(0),ushort2(0),half4(d),traces,p);
}
// No color input: proves derivatives of the known spatial function independently
// of attachment fetch and provides an exact active-output reference at edges.
fragment Colors analytic_derivative(Raster r [[stage_in]], device Trace *traces [[buffer(0)]],
                                    constant Params &p [[buffer(1)]]) {
    float4 pd,vd;independent(r,pd,vd);
    half2 prior=analytic(r.position.xy,p.seed),x=dfdx(prior),y=dfdy(prior);
    float4 d=float4(float2(x),float2(y));
    return record(r,prior,pd,vd,d,as_type<ushort2>(x),as_type<ushort2>(y),half4(x,y),traces,p);
}
