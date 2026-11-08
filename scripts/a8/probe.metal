// Public API observations of synthetic one-byte textures. No channel repair.
#include <metal_stdlib>
using namespace metal;

// count, dispatch tag, intentionally skipped row (UINT_MAX for ordinary runs), reserved
struct Params { uint count; uint tag; uint skip; uint reserved; };

kernel void read_pair(texture2d<float, access::read> f [[texture(0)]],
                      texture2d<half, access::read> h [[texture(1)]],
                      device uint4 *fo [[buffer(0)]],
                      device ushort4 *ho [[buffer(1)]],
                      device uint4 *marks [[buffer(2)]],
                      constant Params &p [[buffer(3)]],
                      uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    uint2 xy = uint2(i % f.get_width(), i / f.get_width());
    fo[i] = as_type<uint4>(f.read(xy));
    ho[i] = as_type<ushort4>(h.read(xy));
    marks[i] = uint4(0xa801c0deu, i, p.tag, ~i);
}

kernel void sample_pair(texture2d<float, access::sample> f [[texture(0)]],
                        texture2d<half, access::sample> h [[texture(1)]],
                        sampler near [[sampler(0)]], sampler linear [[sampler(1)]],
                        const device uint2 *uv_bits [[buffer(0)]],
                        device uint4 *nf [[buffer(1)]],
                        device ushort4 *nh [[buffer(2)]],
                        device uint4 *lf [[buffer(3)]],
                        device ushort4 *lh [[buffer(4)]],
                        device uint4 *marks [[buffer(5)]],
                        constant Params &p [[buffer(6)]],
                        uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    float2 uv = as_type<float2>(uv_bits[i]);
    nf[i] = as_type<uint4>(f.sample(near, uv, level(0)));
    nh[i] = as_type<ushort4>(h.sample(near, uv, level(0)));
    lf[i] = as_type<uint4>(f.sample(linear, uv, level(0)));
    lh[i] = as_type<ushort4>(h.sample(linear, uv, level(0)));
    marks[i] = uint4(0xa801c0deu, i, p.tag, ~i);
}
