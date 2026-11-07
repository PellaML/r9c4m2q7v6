// Newly authored synthetic framebuffer-fetch/derivative observation.
#include <metal_stdlib>
using namespace metal;
struct Params { float scale, bias, marker; uint writeMode; uint base, width, height, unused; };
struct VOut { float4 position [[position]]; };
struct Out { float4 color [[color(0)]]; float4 halfDerivatives [[color(1)]]; float4 floatStages [[color(2)]]; };
struct Trace { uint w[24]; };
vertex VOut vertex_main(uint id [[vertex_id]], constant float2* points [[buffer(0)]]) {
    float2 p=points[id]; VOut o;
    o.position=float4(p.x/16.0f-1.0f, 1.0f-p.y/16.0f, 0.0f, 1.0f); return o;
}
inline Out observe(VOut r, float4 prior, uint primitive, constant Params& p, device Trace* traces) {
    half ah=half(prior.a);
    float f=fma(p.scale,float(ah),p.bias);
    half h=half(f);
    half dx=dfdx(h), dy=dfdy(h), fw=fwidth(h);
    half separate=half(abs(dx)+abs(dy));
    float dxF=dfdx(float(h)), dyF=dfdy(float(h)), fwF=fwidth(float(h));
    half numerator=half(h-half(0.5f));
    half denominator=max(fw,half(0.00390625f));
    half quotient=half(numerator/denominator);
    half effect=clamp(half(quotient+half(0.5f)),half(0),half(1));
    uint x=uint(r.position.x), y=uint(r.position.y);
    uint c=((x*37u+y*73u+x*y*19u)^(x<<3)^(y<<5))&255u;
    half control=half(float(c)/256.0f);
    half cx=dfdx(control), cy=dfdy(control), cw=fwidth(control);
    half oa=ah;
    if(p.writeMode==1u) oa=half(half(1)-ah);
    if(p.writeMode==2u) oa=half(p.marker+float(primitive)*0.1875f);
    if(p.writeMode==3u) oa=effect;
    Out o;
    o.color=p.writeMode==0u ? prior : float4(float(effect),float(ah),float(clamp(fw,half(0),half(1))),float(oa));
    o.halfDerivatives=float4(float(h),float(dx),float(dy),float(fw));
    o.floatStages=float4(float(ah),f,dxF,dyF);
    Trace t={}; t.w[0]=0xfeed0000u|((y*p.width+x)+1u); t.w[1]=primitive;
    float a[22]={prior.a,float(ah),f,float(h),float(dx),float(dy),float(fw),float(separate),
                 dxF,dyF,fwF,float(effect),o.color.a,float(control),float(cx),float(cy),float(cw),
                 r.position.x,r.position.y,prior.r,prior.g,prior.b};
    for(uint j=0;j<22;j++) t.w[j+2]=as_type<uint>(a[j]);
    // Only covered fragments can commit these stores. Each primitive has a separate plane.
    traces[p.base+primitive*p.width*p.height+y*p.width+x]=t;
    return o;
}
fragment Out fetch_float(VOut r [[stage_in]], float4 prior [[color(0)]], uint primitive [[primitive_id]],
                         constant Params& p [[buffer(0)]], device Trace* traces [[buffer(1)]]) {
    return observe(r,prior,primitive,p,traces);
}
fragment Out fetch_half(VOut r [[stage_in]], half4 prior [[color(0)]], uint primitive [[primitive_id]],
                        constant Params& p [[buffer(0)]], device Trace* traces [[buffer(1)]]) {
    return observe(r,float4(prior),primitive,p,traces);
}
