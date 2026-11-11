// Synthetic public scalar framebuffer-fetch experiment. No extracted shader code.
#include <metal_stdlib>
using namespace metal;

struct Params {
    uint caseID, drawID, mode, phase, base, format, diagonal, winding;
    uint rx, ry, rw, rh, sx, sy, sw, sh;
    uint scaleBits, reserved0, reserved1, reserved2;
};
struct Vertex { float4 position [[position]]; };
struct Output { half4 color [[color(0)]]; half2 discarded [[color(1)]]; };
struct Trace { uint w[24]; };

vertex Vertex half_vertex(uint id [[vertex_id]], constant float2 *points [[buffer(0)]]) {
    float2 p = points[id];
    return {float4(p.x / 32.0f - 1.0f, 1.0f - p.y / 32.0f, 0.0f, 1.0f)};
}
inline uint pair(half a, half b) {
    return uint(as_type<ushort>(a)) | (uint(as_type<ushort>(b)) << 16);
}
inline half replacement(uint x, uint y, constant Params& p) {
    if (p.format == 1) return half((x + y + p.phase) & 1u);
    ushort b = ushort(0x4400u + (((3u*x + 5u*y + 7u*p.phase) & 7u) << 6));
    b |= ushort(((x + y + p.phase) & 1u) << 15);
    return as_type<half>(b); // exactly representable signed normal 4 .. 5.75
}

fragment Output half_fetch(Vertex v [[stage_in]], half4 prior [[color(0)]],
                           uint primitive [[primitive_id]], constant Params& p [[buffer(0)]],
                           device Trace *traces [[buffer(1)]],
                           const device half *snapshot [[buffer(2)]],
                           constant float *rounding [[buffer(3)]]) {
    int x = int(v.position.x), y = int(v.position.y);
    uint pixel = uint(clamp(y, 0, 63) * 64 + clamp(x, 0, 63));
    half scale = as_type<half>(ushort(p.scaleBits));
    half h = half(prior.a * scale);
    half dx = dfdx(h), dy = dfdy(h), fw = fwidth(h); // DIRECT HALF operation
    half separate = half(abs(dx) + abs(dy));
    half late = half(half(fw + half(0.25h)) * half(0.5h));
    half marker = replacement(uint(x), uint(y), p);
    half oa = select(prior.a, marker, p.mode != 0u);
    Output out = {half4(late, half(0), half(1), oa), half2(fw, late)};

    // These are native evaluations of explicitly MODELED operands, not reads of
    // native helper lanes. They never feed color0, or the direct-fetch stages.
    half s = snapshot[pixel];
    bool rect = x >= int(p.rx) && y >= int(p.ry) &&
                x < int(p.rx+p.rw) && y < int(p.ry+p.rh);
    bool scissor = x >= int(p.sx) && y >= int(p.sy) &&
                   x < int(p.sx+p.sw) && y < int(p.sy+p.sh);
    int xx = 2*x + 1 - 2*int(p.rx), yy = 2*y + 1 - 2*int(p.ry);
    int side = p.diagonal == 0u ? xx*int(p.rh) + yy*int(p.rw) - 2*int(p.rw*p.rh)
                               : yy*int(p.rw) - xx*int(p.rh);
    bool visible = rect && scissor && p.mode != 0u;
    half forward = select(s, marker, visible && primitive == 1u && side <= 0);
    half reverse = select(s, marker, visible && primitive == 0u && side > 0);
    half live = select(s, marker, visible);
    half scissorMasked = select(half(0), s, scissor);
    half rectangleMasked = select(half(0), s, rect);
    half sfw = fwidth(half(s * scale)), ffw = fwidth(half(forward * scale));
    half rfw = fwidth(half(reverse * scale)), lfw = fwidth(half(live * scale));
    half cfw = fwidth(half(scissorMasked * scale)), bfw = fwidth(half(rectangleMasked * scale));
    float rf = rounding[(uint(x) + 3u*uint(y)) & 31u];
    half rh = half(rf);
    float fh = float(h);

    Trace t = {};
    t.w[0] = 0xa7000000u | (p.caseID << 16) | (p.drawID << 12) | pixel;
    t.w[1] = primitive;
    t.w[2] = pair(prior.r, prior.g); t.w[3] = pair(prior.b, prior.a);
    t.w[4] = pair(dx, dy); t.w[5] = pair(fw, separate);
    t.w[6] = pair(out.color.r, out.color.g); t.w[7] = pair(out.color.b, out.color.a);
    t.w[8] = pair(s, sfw); t.w[9] = pair(ffw, rfw);
    t.w[10] = pair(lfw, cfw); t.w[11] = pair(bfw, late);
    t.w[12] = as_type<uint>(rf); t.w[13] = uint(as_type<ushort>(rh));
    t.w[14] = as_type<uint>(dfdx(fh)); t.w[15] = as_type<uint>(dfdy(fh));
    t.w[16] = as_type<uint>(fwidth(fh));
    t.w[17] = as_type<uint>(v.position.x); t.w[18] = as_type<uint>(v.position.y);
    t.w[19] = pair(h, scale);
    float tiny = rounding[30u + ((uint(x) + uint(y)) & 1u)];
    t.w[20] = as_type<uint>(tiny);
    t.w[21] = as_type<uint>(tiny * 0.5f); // runtime binary32 denorm control
    t.w[22] = 0u;
    uint hash = 2166136261u;
    for (uint j = 0; j < 23; ++j) hash = (hash ^ t.w[j]) * 16777619u;
    t.w[23] = hash;
    // Only covered fragments commit stores. Separate primitive/draw planes;
    // unstamped records supply NO observation about native helper operands.
    traces[p.base + primitive*4096u + pixel] = t;
    return out;
}
