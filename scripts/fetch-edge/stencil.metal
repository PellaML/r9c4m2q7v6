// Owned dyadic framebuffer-fetch experiment. No external shader or data input.
#include <metal_stdlib>
using namespace metal;

struct Raster { float4 position [[position]]; };
struct Params { uint draw_tag, unused0, unused1, unused2; };
struct Outputs {
    half4 color [[color(0)]];
    float4 operand_and_difference [[color(1)]];
    float4 width_and_tags [[color(2)]];
};

vertex Raster stencil_vertex(uint id [[vertex_id]], constant float2* points [[buffer(0)]]) {
    float2 p=points[id];
    return {float4(p.x/16.0f-1.0f, 1.0f-p.y/16.0f, 0.0f, 1.0f)};
}

inline Outputs observe_stencil(Raster r, half4 prior, uint primitive, constant Params& p) {
    // Binary alpha seeds make every arithmetic stage exactly dyadic. The shader
    // is straight-line: no discard or device-buffer side effects.
    half h=half(0.25h+0.5h*prior.a);
    half dx=dfdx(h), dy=dfdy(h), fw=fwidth(h);
    half separate=half(abs(dx)+abs(dy));
    Outputs out;
    out.color=half4(1.0h-prior.r,prior.g,prior.b,1.0h-prior.a);
    out.operand_and_difference=float4(float(prior.a),float(h),float(dx),float(dy));
    out.width_and_tags=float4(float(fw),float(separate),float(primitive),float(p.draw_tag));
    return out;
}

fragment Outputs stencil_default(Raster r [[stage_in]], half4 prior [[color(0)]],
    uint primitive [[primitive_id]], constant Params& p [[buffer(0)]]) {
    return observe_stencil(r,prior,primitive,p);
}

[[early_fragment_tests]]
fragment Outputs stencil_early(Raster r [[stage_in]], half4 prior [[color(0)]],
    uint primitive [[primitive_id]], constant Params& p [[buffer(0)]]) {
    return observe_stencil(r,prior,primitive,p);
}

// Essential topology control: only BGRA color0 can store. Color1 is bound RG16,
// declared half2, and physically write-masked off; there is no color2 target.
// Derivatives determine visible RGB bytes, so their work cannot be discarded
// merely because an observation MRT is unused.
struct VisibleOutputs { half4 color [[color(0)]]; half2 masked [[color(1)]]; };
[[early_fragment_tests]]
fragment VisibleOutputs stencil_visible_early(half4 prior [[color(0)]]) {
    half h=half(0.25h+0.5h*prior.a);
    half dx=dfdx(h), dy=dfdy(h), fw=fwidth(h);
    return {half4(0.5h+dx,0.5h+dy,fw,1.0h-prior.a),half2(0.0h)};
}

// Stencil is observed by an independent Equal/Keep GPU sweep, not by a CPU
// renderer, a sampled color stand-in, or an assumed stencil-blit representation.
fragment uint4 stencil_query(constant uint4& p [[buffer(0)]]) {
    return uint4(p.x,255u-p.x,0x53u,255u);
}
