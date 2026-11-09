// Independently chosen mathematical inputs only; no application shader or data.
#include <metal_stdlib>
using namespace metal;

struct U16Row { ushort result, guard0, input, guard1; uint stamp; };
static_assert(sizeof(U16Row) == 12 && __builtin_offsetof(U16Row, input) == 4 &&
              __builtin_offsetof(U16Row, stamp) == 8, "scalar output layout");

kernel void scalar_half_ushort(const device ushort *input [[buffer(0)]],
                               device U16Row *output [[buffer(1)]],
                               constant uint2 &params [[buffer(2)]],
                               uint i [[thread_position_in_grid]]) {
    if (i >= params.x) return;
    half x = as_type<half>(input[i]);
    ushort y = ushort(x); // Must be a LIVE scalar air.convert.u.i16.f.f16 in AIR.
    output[i].result = y;
    output[i].input = as_type<ushort>(x);
    output[i].stamp = 0x4d310000u | (params.y << 8) | i;
}

struct Vertex { float4 position; float4 payload; };
static_assert(sizeof(Vertex) == 32, "vertex buffer layout");
struct Linked {
    float4 position [[position]];
    float3 perspectiveValue [[center_perspective]];
    float3 linearValue [[center_no_perspective]];
};
struct Bits { uint4 a [[color(0)]]; uint4 b [[color(1)]]; };

vertex Linked large_vertex(const device Vertex *vertices [[buffer(0)]],
                           uint i [[vertex_id]]) {
    Linked r;
    r.position = vertices[i].position;
    // Buffer loads prevent source constant folding. Both are genuine f32 links.
    r.perspectiveValue = vertices[i].payload.xyz;
    r.linearValue = vertices[i].payload.xyz;
    return r;
}

fragment Bits large_fragment(Linked r [[stage_in]], constant uint &tag [[buffer(0)]]) {
    uint2 xy = uint2(r.position.xy);
    Bits out;
    out.a = uint4(as_type<uint>(r.perspectiveValue.x), as_type<uint>(r.linearValue.x),
                  as_type<uint>(r.perspectiveValue.y), as_type<uint>(r.perspectiveValue.z));
    out.b = uint4(as_type<uint>(r.linearValue.y), as_type<uint>(r.linearValue.z),
                  xy.x | (xy.y << 16), 0x4d320000u | tag | (xy.y * 4u + xy.x));
    return out; // RGBA32Uint x 2: raw f32 bits, no half/UNORM/float export conversion.
}
