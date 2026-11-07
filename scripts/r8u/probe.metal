// Newly authored synthetic scalar-UNORM storage, sampling and blending probe.
#include <metal_stdlib>
using namespace metal;
struct VertexOut { float4 p [[position]]; };
vertex VertexOut triangle(uint i [[vertex_id]]) {
    VertexOut o;o.p=float4(i==1 ? 3.0f : -1.0f,i==2 ? 3.0f : -1.0f,0,1);return o;
}
struct Params { uint width; uint flipAlpha; };
struct FloatOut { float4 color [[color(0)]]; float4 emitted [[color(1)]]; };
struct HalfOut { half4 color [[color(0)]]; float4 emitted [[color(1)]]; };
fragment FloatOut output_float(VertexOut v [[stage_in]], const device float4 *src [[buffer(0)]], constant Params &p [[buffer(1)]]) {
    uint i=uint(v.p.y)*p.width+uint(v.p.x);float4 s=src[i];
    if(p.flipAlpha) s.a=1.0f-s.a;
    FloatOut o;o.color=s;o.emitted=s;return o;
}
fragment HalfOut output_half(VertexOut v [[stage_in]], const device float4 *src [[buffer(0)]], constant Params &p [[buffer(1)]]) {
    uint i=uint(v.p.y)*p.width+uint(v.p.x);float4 f=src[i];
    if(p.flipAlpha) f.a=1.0f-f.a;
    half4 s=half4(f);HalfOut o;o.color=s;o.emitted=float4(s);return o;
}
kernel void direct_read(texture2d<float, access::read> f [[texture(0)]], texture2d<half, access::read> h [[texture(1)]],
                        device uint4 *fo [[buffer(0)]], device ushort4 *ho [[buffer(1)]], uint2 q [[thread_position_in_grid]]) {
    if(q.x>=f.get_width()||q.y>=f.get_height())return;
    uint i=q.y*f.get_width()+q.x;fo[i]=as_type<uint4>(f.read(q));ho[i]=as_type<ushort4>(h.read(q));
}
kernel void sample_float(texture2d<float, access::sample> t [[texture(0)]], sampler near [[sampler(0)]], sampler linear [[sampler(1)]],
                         const device float2 *uv [[buffer(0)]], device uint4 *n [[buffer(1)]],device uint4 *l [[buffer(2)]],
                         constant uint &count [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if(i>=count)return; n[i]=as_type<uint4>(t.sample(near,uv[i],level(0))); l[i]=as_type<uint4>(t.sample(linear,uv[i],level(0)));
}
kernel void sample_half(texture2d<half, access::sample> t [[texture(0)]], sampler near [[sampler(0)]], sampler linear [[sampler(1)]],
                        const device float2 *uv [[buffer(0)]], device ushort4 *n [[buffer(1)]],device ushort4 *l [[buffer(2)]],
                        constant uint &count [[buffer(3)]], uint i [[thread_position_in_grid]]) {
    if(i>=count)return; n[i]=as_type<ushort4>(t.sample(near,uv[i],level(0))); l[i]=as_type<ushort4>(t.sample(linear,uv[i],level(0)));
}
