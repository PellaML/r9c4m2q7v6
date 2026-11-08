#include <metal_stdlib>
using namespace metal;
struct Out { float4 position [[position]]; uint group [[flat]]; uint instance [[flat]]; };
vertex Out indexed_vertices(uint id [[vertex_id]], uint instance [[instance_id]],
                            const device uint4 *positions [[buffer(0)]]) {
    // Both large IDs are deliberately valid shader inputs. Never an OOB fetch.
    uint slot = id == 65535u ? 13u : (id == 65534u ? 14u : id);
    Out out;
    out.position = as_type<float4>(positions[slot]);
    out.position.y -= float(instance); // Two disjoint 8-pixel-high instance bands.
    out.group = slot < 5u ? 1u : (slot < 9u ? 2u : (slot < 13u ? 3u : 4u));
    out.instance = instance;
    return out;
}
fragment uint4 primitive_bits(Out in [[stage_in]], bool front [[front_facing]],
                              constant uint &skipRow [[buffer(0)]]) {
    if (uint(in.position.y) == skipRow) { discard_fragment(); return uint4(0u); }
    return uint4(uint(front), 160u + in.group, in.instance + 1u, 215u);
}
