#!/usr/bin/env python3
"""Owned quad inputs and hard render/coverage/known-plane controls. No half model is assumed."""
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import sys

WIDTH, HEIGHT, CASES, PASSES, CONTROLS = 128, 64, 2048, 6, 64
PIXELS = WIDTH * HEIGHT
INPUT, TRACE, RGBA = struct.Struct("<8I12H2I"), struct.Struct("<9I12HI"), struct.Struct("<4I")
FILES = ("inputs.bin", "corpus.json", "trace.bin", "coverage.bin", "textures.bin", "native.json",
         "check.json", "wrong-expected.json", "toolchain.txt", "derivative.air", "derivative.metallib")


def fbits(x):
    return struct.unpack("<I", struct.pack("<f", x))[0]


def fvalue(x):
    return struct.unpack("<f", struct.pack("<I", x))[0]


def hbits(x):
    return struct.unpack("<H", struct.pack("<e", x))[0]


def widen(x):
    return fbits(struct.unpack("<e", struct.pack("<H", x))[0])


def inputs():
    quads, labels = [], []
    def add(label, words):
        assert len(words) == 4 and all(w & 0x7C00 != 0x7C00 for w in words)
        quads.append(tuple(words)); labels.append(label)
    for mask in range(16):
        add(f"signed_zero_{mask}", [0x8000 if mask & (1 << lane) else 0 for lane in range(4)])
    for words in ([0,1,2,3], [0x8000,0x8001,0x8002,0x8003], [0x8001,1,0x8002,2],
                  [0x03FE,0x03FF,0x0400,0x0401], [0x83FE,0x83FF,0x8400,0x8401],
                  [0x3BFF,0x3C00,0x3C01,0x3C02], [0,0x3C00,0x4400,0x4A00]):
        add("subnormal_boundary_or_nonaffine", words)
        add("transposed", [words[0],words[2],words[1],words[3]])
    # Shared-origin sums distinguish rounded half differences from a wide sum.
    for epsilon in (0x0FFF,0x1000,0x1001):
        for sign in (False, True):
            words = [epsilon | (0 if sign else 0x8000), 0xBC00 if sign else 0x3C00,
                     0x8000 if sign else 0, 0xBC00 if sign else 0x3C00]
            add("difference_rounding_discriminator", words)
            add("difference_rounding_transposed", [words[0],words[2],words[1],words[3]])
    for values in ((-15,65504,-15,65504),(-16,65504,-16,65504),(-17,65504,-17,65504),
                   (-65504,65504,-65504,65504),(65504,-65504,-65504,65504),
                   (0,40000,40000,60000),(65504,65472,-65504,-65472)):
        add("finite_input_overflow_boundary", [hbits(v) for v in values])
    for word in (0,0x8000,1,0x8001,0x03FF,0x83FF,0x0400,0x8400,0x3C00,0xBC00,0x7BFF,0xFBFF):
        add("constant_quad", [word]*4)
    curated_count = len(quads)
    state = 0xA341316C
    def next_word():
        nonlocal state
        state ^= (state << 13) & 0xFFFFFFFF; state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF; state &= 0xFFFFFFFF
        return state
    while len(quads) < CASES:
        words = [next_word() & 0xFFFF for _ in range(4)]
        add("deterministic_finite", [w ^ (0x0400 if w & 0x7C00 == 0x7C00 else 0) for w in words])
    rows = []
    for case in range(CASES):
        f = []
        for lane in range(4):
            x = 2*(case % (WIDTH//2)) + lane%2
            y = 2*(case // (WIDTH//2)) + lane//2
            if case < CONTROLS:
                values = (16 + 2*x + 4*y, 32 - 8*x + 2*y)
            elif case == CONTROLS:
                values = ((0,1,4,12)[lane], (0,2,9,20)[lane])
            else:
                values = ((int(next_word() % 4097)-2048)/4, (int(next_word() % 8193)-4096)/8)
            f.extend(fbits(v) for v in values)
        second = quads[(73*case+17) % CASES]
        vector = [word for lane in range(4) for word in (quads[case][lane],second[lane])]
        rows.append(tuple(f + list(quads[case]) + vector + [0,0]))
    return rows, {"curated_count":curated_count,
        "curated_half_quads":[{"case":i,"label":labels[i],"bits":[f"{x:04x}" for x in quads[i]]}
                              for i in range(curated_count)]}


def write_json(path, value):
    path.write_text(json.dumps(value,indent=2,sort_keys=True)+"\n",encoding="utf-8")


def generate(out):
    out.mkdir(parents=True,exist_ok=False)
    rows, info = inputs()
    data = b"".join(INPUT.pack(*r) for r in rows)
    (out/"inputs.bin").write_bytes(data)
    write_json(out/"corpus.json", {"schema":1,"width":WIDTH,"height":HEIGHT,"quad_cases":CASES,
        "passes":PASSES,"input_stride":INPUT.size,"trace_stride":TRACE.size,
        "lane_order":"(x_even,y_even),(x_odd,y_even),(x_even,y_odd),(x_odd,y_odd)",
        "known_f32_control_cases":CONTROLS,"control_dx_magnitudes":[2,8],"control_dy_magnitudes":[4,2],
        "control_fwidth":[6,10],"half_vector_x_equals_scalar":True,
        "half_vector_y_from_case":"(73*case+17)%2048","half_numeric_models":"diagnostic only; no expected half answers sent to GPU",
        "inputs_sha256":hashlib.sha256(data).hexdigest(),"compiler_math":"fast",**info})


def known_errors(actual, wrong=False):
    expected = list(map(fbits,(2,8,4,2,6,10)))
    if wrong: expected[4] ^= 1
    return sum((word & 0x7FFFFFFF if c < 4 else word) != expected[c] for c,word in enumerate(actual))


def examine(rows, traces, coverage, textures, wrong):
    identity = coverage_errors = positive_errors = 0
    positive_cells = 0
    for linear,t in enumerate(traces):
        x,y = linear%WIDTH,linear//WIDTH
        case,lane = (y//2)*(WIDTH//2)+x//2,(y%2)*2+x%2
        r = rows[case]
        identity += not (t[0] == (linear+1)^0x52D31AB7 and t[1:3] == r[2*lane:2*lane+2]
            and t[9] == r[8+lane] and t[13:15] == r[12+2*lane:14+2*lane] and t[21] == 0)
        for mode in range(PASSES):
            coverage_errors += coverage[mode*PIXELS+linear] != ((mode+1)<<24)|(linear+1)
        if case < CONTROLS:
            positive_errors += known_errors(t[3:9], wrong and linear == 0)
            positive_cells += 6
    # Position readback identifies the texture-to-screen orientation rather than assuming it.
    mapping, ids = [], set()
    for pixel in range(PIXELS):
        pos = textures[PIXELS+pixel][2:4]
        px,py = map(fvalue,pos)
        if not (math.isfinite(px) and math.isfinite(py) and 0 <= px < WIDTH and 0 <= py < HEIGHT
                and px == math.floor(px)+0.5 and py == math.floor(py)+0.5):
            raise ValueError("invalid pixel-center readback")
        linear = int(py)*WIDTH+int(px)
        if linear in ids: raise ValueError("duplicate pixel-center readback")
        ids.add(linear); mapping.append(linear)
        x,y = int(px),int(py)
        case,lane = (y//2)*(WIDTH//2)+x//2,(y%2)*2+x%2
        identity += textures[5*PIXELS+pixel][2:] != (fbits(case+1),fbits(lane+1))
        if case < CONTROLS:
            actual = (*textures[pixel],*textures[PIXELS+pixel][:2])
            positive_errors += known_errors(actual)
            positive_cells += 6
    if len(ids) != PIXELS: raise ValueError("incomplete coverage")
    # Numerical agreement between raw trace and widened RGBA32Float is diagnostic,
    # particularly for zero signs and overflowing fast-half intermediates.
    conversion_errors, first = 0, []
    for pixel,linear in enumerate(mapping):
        t=traces[linear]
        expected=(t[3:7],t[7:9],tuple(widen(h) for h in (t[10],t[11],t[12],t[9])),
                  tuple(widen(h) for h in t[15:19]),tuple(widen(h) for h in (*t[19:21],*t[13:15])),t[1:3])
        for mode,e in enumerate(expected):
            a=textures[mode*PIXELS+pixel][:len(e)]
            for channel,(want,got) in enumerate(zip(e,a)):
                if want != got:
                    conversion_errors += 1
                    if len(first)<8: first.append({"pixel":pixel,"position_index":linear,"pass":mode,
                        "channel":channel,"trace_widened":f"{want:08x}","texture":f"{got:08x}"})
    orientation = "identity" if mapping == list(range(PIXELS)) else (
        "flip_y" if mapping == [(HEIGHT-1-p//WIDTH)*WIDTH+p%WIDTH for p in range(PIXELS)] else "other_bijection")
    return {"schema":1,"identity_errors":identity,"coverage_errors":coverage_errors,
        "covered_fragments":PIXELS*PASSES-coverage_errors,"full_quads_per_pass":CASES,
        "pixel_centers_unique":len(ids),"texture_to_position":orientation,
        "positive_f32_control_cells":positive_cells,"positive_control_errors":positive_errors,
        "wrong_expected_control":wrong,"texture_trace_bit_differences_diagnostic":conversion_errors,
        "first_texture_trace_differences":first,
        "half_model_acceptance_claimed":False,
        "hard_controls_pass":not(identity or coverage_errors or positive_errors)}


def check(out,wrong):
    rows,_=inputs()
    if (out/"inputs.bin").read_bytes()!=b"".join(INPUT.pack(*r) for r in rows):
        raise ValueError("input identity")
    trace=(out/"trace.bin").read_bytes(); cov=(out/"coverage.bin").read_bytes(); tex=(out/"textures.bin").read_bytes()
    if len(trace)!=PIXELS*64 or len(cov)!=PASSES*PIXELS*4 or len(tex)!=PASSES*PIXELS*16:
        raise ValueError("readback size")
    native=json.loads((out/"native.json").read_text())
    if not (native["availability"]=="available_and_executed" and native["command_status"]==4
            and native["error"] is None and native["pipeline_created"] and native["wait_until_completed_called"]
            and native["readback_copied_after_successful_wait"] and native["render_passes"]==PASSES
            and native["draw_calls"]==PASSES and native["raster_sample_count"]==1
            and native["width"]==WIDTH and native["height"]==HEIGHT and native["pixel_format"]=="RGBA32Float"):
        raise ValueError("native render completion join")
    result=examine(rows,list(TRACE.iter_unpack(trace)),[v[0] for v in struct.iter_unpack("<I",cov)],
                   list(RGBA.iter_unpack(tex)),wrong)
    result["trace_sha256"]=hashlib.sha256(trace).hexdigest()
    result["textures_sha256"]=hashlib.sha256(tex).hexdigest()
    write_json(out/("wrong-expected.json" if wrong else "check.json"),result)
    if wrong and not(result["positive_control_errors"]==1 and result["identity_errors"]==0 and result["coverage_errors"]==0):
        raise ValueError("wrong-expected control must reject exactly one known cell")
    return 0 if result["hard_controls_pass"] else 1


def audit(out):
    root,home=str(Path.cwd().resolve()),os.environ.get("HOME","")
    repository=os.environ.get("GITHUB_REPOSITORY","").split("/")
    public=(len(repository)==2 and all(repository) and os.environ.get("GITHUB_ACTIONS")=="true"
        and os.environ.get("RUNNER_ENVIRONMENT")=="github-hosted" and home=="/Users/runner"
        and root==f"/Users/runner/work/{repository[-1]}/{repository[-1]}"
        and os.environ.get("GITHUB_WORKSPACE")==root)
    for name in FILES:
        data=(out/name).read_bytes()
        if not 0<len(data)<=4*1024*1024: raise ValueError("artifact size")
        if not public:
            for path in (root,home):
                if len(path)>1 and any(path.encode(e) in data for e in ("utf-8","utf-16le","utf-16be")):
                    raise ValueError("nonpublic local path")


def selftest():
    rows,_=inputs()
    assert INPUT.size==64 and TRACE.size==64 and len(rows)==CASES
    assert all(h&0x7C00!=0x7C00 for r in rows for h in r[8:20])
    assert all(r[8+lane]==r[12+2*lane] for r in rows for lane in range(4))
    for case in range(CONTROLS):
        r=rows[case]; v=[[fvalue(r[2*l+c]) for c in range(2)] for l in range(4)]
        dx=[v[1][c]-v[0][c] for c in range(2)]
        dy=[v[2][c]-v[0][c] for c in range(2)]
        actual=list(map(fbits,dx+dy+[abs(dx[c])+abs(dy[c]) for c in range(2)]))
        assert known_errors(actual)==0 and known_errors(actual,True)==1
    assert known_errors([0]*6)==6
    print("owned finite corpus, layouts, exact nonzero F32 planes and negative control passed")


def main():
    if sys.argv[1:]==["selftest"]: selftest(); return 0
    if len(sys.argv)!=3: raise ValueError("usage: selftest | generate/check/wrong-expected/audit DIRECTORY")
    command,out=sys.argv[1],Path(sys.argv[2])
    if command=="generate": generate(out); return 0
    if command=="check": return check(out,False)
    if command=="wrong-expected": return check(out,True)
    if command=="audit": audit(out); return 0
    raise ValueError("unknown operation")


if __name__=="__main__":
    try: status=main()
    except (ValueError,OSError,AssertionError,KeyError,struct.error):
        print("derivative probe refused: render, coverage, control or privacy check failed",file=sys.stderr)
        status=3
    sys.exit(status)
