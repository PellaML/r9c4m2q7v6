// Owned API capture only: no format conversion or filtering model.
#include <metal_stdlib>
using namespace metal;
struct Params { uint count, tag, mode, pixel, skip; };
struct Row { uint4 rf; ushort4 rh; ushort4 ch; uint4 cf; uint4 mark; };
static_assert(sizeof(Row) == 64, "capture row layout");
constexpr sampler s0(coord::normalized, filter::nearest, address::clamp_to_edge, mip_filter::none);
constexpr sampler s1(coord::normalized, filter::linear,  address::clamp_to_edge, mip_filter::none);
constexpr sampler s3(coord::pixel,      filter::linear,  address::clamp_to_edge, mip_filter::none);
constexpr sampler s5(coord::normalized, filter::linear,  address::clamp_to_zero, mip_filter::none);
constexpr sampler s7(coord::pixel,      filter::linear,  address::clamp_to_zero, mip_filter::none);
#define STATIC_SAMPLE(T) \
T##4 fixed_##T(texture2d<T, access::sample> t, float2 uv, uint mode) { \
    switch (mode) { \
    case 0: return t.sample(s0, uv, level(0.0f)); case 1: return t.sample(s1, uv, level(0.0f)); \
    case 3: return t.sample(s3, uv, level(0.0f)); case 5: return t.sample(s5, uv, level(0.0f)); \
    default: return t.sample(s7, uv, level(0.0f)); } }
STATIC_SAMPLE(float)
STATIC_SAMPLE(half)
// Generic payload integrity tag, NOT a numerical prediction or format model.
uint payload_tag(Row r) {
    uint h = 2166136261u;
    for (uint j = 0; j < 4; ++j) h = (h ^ r.rf[j]) * 16777619u;
    uint2 a = as_type<uint2>(r.rh), b = as_type<uint2>(r.ch);
    for (uint j = 0; j < 2; ++j) h = (h ^ a[j]) * 16777619u;
    for (uint j = 0; j < 2; ++j) h = (h ^ b[j]) * 16777619u;
    for (uint j = 0; j < 4; ++j) h = (h ^ r.cf[j]) * 16777619u;
    return h ^ 0x53414d50u;
}
kernel void sample_capture(texture2d<float> f [[texture(0)]], texture2d<half> h [[texture(1)]],
                           sampler s [[sampler(0)]], const device uint4 *coords [[buffer(0)]],
                           device Row *out [[buffer(1)]], constant Params &p [[buffer(2)]],
                           uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    float2 uv = as_type<float2>(p.pixel ? coords[i].xy : coords[i].zw);
    Row r = {};
    r.rf = as_type<uint4>(f.sample(s, uv, level(0.0f)));
    r.rh = as_type<ushort4>(h.sample(s, uv, level(0.0f)));
    r.cf = as_type<uint4>(fixed_float(f, uv, p.mode));
    r.ch = as_type<ushort4>(fixed_half(h, uv, p.mode));
    r.mark = uint4(payload_tag(r), i, p.tag, ~i); out[i] = r;
}
kernel void read_capture(texture2d<float, access::read> f [[texture(0)]],
                         texture2d<half, access::read> h [[texture(1)]],
                         device Row *out [[buffer(1)]], constant Params &p [[buffer(2)]],
                         uint i [[thread_position_in_grid]]) {
    if (i >= p.count) return;
    uint2 xy(i % f.get_width(), i / f.get_width()); Row r = {};
    r.rf = as_type<uint4>(f.read(xy)); r.rh = as_type<ushort4>(h.read(xy));
    r.cf = r.rf; r.ch = r.rh;
    r.mark = uint4(payload_tag(r), i, p.tag, ~i); out[i] = r;
}
