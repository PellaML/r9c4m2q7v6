// Synthetic public MSL conversions. Separate kernels prevent reusing an i32
// conversion as the narrow conversion. Raw integer stores, no render target.
#include <metal_stdlib>
using namespace metal;

struct Params { uint count; uint tag; uint skip; uint reserved; };
struct ShortRow { ushort value; ushort guard0; ushort input; ushort guard1; uint marker; };
struct IntRow { uint value; ushort input; ushort guard0; uint marker; };
struct HalfRow { ushort value; ushort guard0; uint input; uint marker; };
static_assert(sizeof(ShortRow) == 12 && __builtin_offsetof(ShortRow, input) == 4 &&
              __builtin_offsetof(ShortRow, marker) == 8, "short output layout");
static_assert(sizeof(IntRow) == 12 && __builtin_offsetof(IntRow, input) == 4 &&
              __builtin_offsetof(IntRow, marker) == 8, "int output layout");
static_assert(sizeof(HalfRow) == 12 && __builtin_offsetof(HalfRow, input) == 4 &&
              __builtin_offsetof(HalfRow, marker) == 8, "half output layout");

kernel void convert_half_short(const device ushort *input [[buffer(0)]],
                               device ShortRow *out [[buffer(1)]],
                               constant Params &p [[buffer(2)]],
                               uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    half x = as_type<half>(input[i]);
    short y = short(x);
    out[i].value = as_type<ushort>(y);
    out[i].input = as_type<ushort>(x);
    out[i].marker = 0x5a000000u | (p.tag << 16) | i;
}

kernel void convert_half_int(const device ushort *input [[buffer(0)]],
                             device IntRow *out [[buffer(1)]],
                             constant Params &p [[buffer(2)]],
                             uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    half x = as_type<half>(input[i]);
    int y = int(x);
    out[i].value = as_type<uint>(y);
    out[i].input = as_type<ushort>(x);
    out[i].marker = 0x5a000000u | (p.tag << 16) | i;
}

kernel void convert_int_half(const device int *input [[buffer(0)]],
                             device HalfRow *out [[buffer(1)]],
                             constant Params &p [[buffer(2)]],
                             uint i [[thread_position_in_grid]]) {
    if (i >= p.count || i == p.skip) return;
    int x = input[i];
    half y = half(x);
    out[i].value = as_type<ushort>(y);
    out[i].input = as_type<uint>(x);
    out[i].marker = 0x5a000000u | (p.tag << 16) | i;
}
