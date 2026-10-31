#include <metal_stdlib>
using namespace metal;

struct QuadInput { uint f[8]; ushort scalar[4]; ushort vector[8]; uint reserved[2]; };
struct Params { uint width, height, mode, reserved; };
struct Trace {
    uint stamp, f_input[2], f_dx[2], f_dy[2], f_width[2];
    ushort h_input, h_dx, h_dy, h_width;
    ushort hv_input[2], hv_dx[2], hv_dy[2], hv_width[2];
    uint reserved;
};
struct Raster { float4 position [[position]]; };

vertex Raster quad_vertex(uint id [[vertex_id]]) {
    const float2 positions[3] = {float2(-1, -1), float2(3, -1), float2(-1, 3)};
    Raster result;
    result.position = float4(positions[id], 0, 1);
    return result;
}

fragment float4 derivative_probe(Raster raster [[stage_in]],
    device const QuadInput *inputs [[buffer(0)]], constant Params &params [[buffer(1)]],
    device uint *coverage [[buffer(2)]], device Trace *traces [[buffer(3)]]) {
    uint2 pixel = uint2(raster.position.xy);
    uint linear = pixel.y * params.width + pixel.x;
    uint index = (pixel.y / 2) * (params.width / 2) + pixel.x / 2;
    uint lane = (pixel.y & 1u) * 2 + (pixel.x & 1u);
    QuadInput input = inputs[index];
    uint2 fb = uint2(input.f[2 * lane], input.f[2 * lane + 1]);
    ushort2 hb = ushort2(input.vector[2 * lane], input.vector[2 * lane + 1]);
    float2 f = as_type<float2>(fb);
    half s = as_type<half>(input.scalar[lane]);
    half2 v = as_type<half2>(hb);

    // These are fragment-stage derivatives. Mode is uniform for the whole draw.
    float2 fx = dfdx(f), fy = dfdy(f), fw = fwidth(f);
    half sx = dfdx(s), sy = dfdy(s), sw = fwidth(s);
    half2 vx = dfdx(v), vy = dfdy(v), vw = fwidth(v);
    coverage[params.mode * params.width * params.height + linear] =
        ((params.mode + 1) << 24) | (linear + 1);
    if (params.mode == 0) {
        uint2 x = as_type<uint2>(fx), y = as_type<uint2>(fy), w = as_type<uint2>(fw);
        ushort2 hx = as_type<ushort2>(vx), hy = as_type<ushort2>(vy), hw = as_type<ushort2>(vw);
        traces[linear].stamp = (linear + 1) ^ 0x52d31ab7u;
        for (uint c = 0; c < 2; ++c) {
            traces[linear].f_input[c] = fb[c];
            traces[linear].f_dx[c] = x[c];
            traces[linear].f_dy[c] = y[c];
            traces[linear].f_width[c] = w[c];
            traces[linear].hv_input[c] = hb[c];
            traces[linear].hv_dx[c] = hx[c];
            traces[linear].hv_dy[c] = hy[c];
            traces[linear].hv_width[c] = hw[c];
        }
        traces[linear].h_input = input.scalar[lane];
        traces[linear].h_dx = as_type<ushort>(sx);
        traces[linear].h_dy = as_type<ushort>(sy);
        traces[linear].h_width = as_type<ushort>(sw);
        traces[linear].reserved = 0;
    }
    // One RGBA32Float target per pass avoids aggregate MRT-size assumptions.
    switch (params.mode) {
        case 0: return float4(fx, fy);
        case 1: return float4(fw, raster.position.xy);
        case 2: return float4(float(sx), float(sy), float(sw), float(s));
        case 3: return float4(float2(vx), float2(vy));
        case 4: return float4(float2(vw), float2(v));
        default: return float4(f, float(index + 1), float(lane + 1));
    }
}
