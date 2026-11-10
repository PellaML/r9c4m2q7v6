// Owned affine-varying experiment on a real odd-sized framebuffer.
#include <metal_stdlib>
using namespace metal;

struct Raster { float4 position [[position]]; float value; };
struct Output { half4 color [[color(0)]]; half2 masked [[color(1)]]; };

vertex Raster edge_vertex(uint id [[vertex_id]], constant float2* points [[buffer(0)]]) {
    float2 p=points[id];
    Raster out;
    out.position=float4(2.0f*p.x/35.0f-1.0f, 1.0f-2.0f*p.y/27.0f, 0.0f, 1.0f);
    out.value=p.x*0.0625f+p.y*0.03125f;
    return out;
}

// Same fetch, varying and visible arithmetic, without derivative instructions.
[[early_fragment_tests]]
fragment Output edge_scalar(Raster in [[stage_in]], half2 prior [[color(1)]]) {
    float h=in.value;
    float dx=0.0625f, dy=0.03125f;
    float fw=dx+dy, separate=abs(dx)+abs(dy);
    return {half4(float(prior.x)+dx,float(prior.y)+dy,fw,0.25f*h+separate),half2(0.0h)};
}

// Only the interpolated varying enters derivatives. Unknown fetch values in
// non-storing helper lanes cannot enter a derivative operand in this algebra.
[[early_fragment_tests]]
fragment Output edge_derivative(Raster in [[stage_in]], half2 prior [[color(1)]]) {
    float h=in.value;
    float dx=dfdx(h), dy=dfdy(h), fw=fwidth(h);
    float separate=abs(dx)+abs(dy);
    return {half4(float(prior.x)+dx,float(prior.y)+dy,fw,0.25f*h+separate),half2(0.0h)};
}

// Independent GPU Equal/Keep sweep; no assumed stencil-blit representation.
fragment uint4 edge_query(constant uint4& p [[buffer(0)]]) {
    return uint4(p.x,255u-p.x,0x53u,255u);
}
