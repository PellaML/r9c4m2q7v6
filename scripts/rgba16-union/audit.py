"""Bounded byte/metadata integrity checks only. Never predict blended values."""
import hashlib
import json
import re
import struct
import sys
from pathlib import Path

MODES = ("precise", "fast")
SPECS = (("seed", 0), ("half-union", 16), ("half-control", 16),
         ("float-union", 32), ("float-control", 32))
COMMON = {"environment.txt", "commands.txt", "build.txt", "source.sha256", "cases.json", "status.json"}
PER_MODE = {"program.air", "program.ll", "program.metallib", "profile.json", "gpu.json", "native.log",
            "source.bin", "seed.bin", "observations.tsv", "validation.json"}
PER_MODE |= {name + ".raw.bin" for name, _ in SPECS}
PER_MODE |= {name + ".trace.bin" for name, bits in SPECS if bits}
ALLOWED = COMMON | {mode + "/" + name for mode in MODES for name in PER_MODE}


def need(ok, reason):
    if not ok:
        raise ValueError(reason)


def read(path):
    need(path.is_file() and not path.is_symlink() and path.stat().st_size <= 1048576,
         "regular bounded file required: " + str(path))
    return path.read_bytes()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(read(path))


def save(path, obj):
    with path.open("x", encoding="utf-8", newline="\n") as f:
        json.dump(obj, f, indent=2, sort_keys=True)
        f.write("\n")


def profile(root, mode):
    text = read(root / "program.ll").decode("utf-8")
    text = "\n".join(line.split(";", 1)[0] for line in text.splitlines())
    nodes = dict(re.findall(r"^!(\d+) = (.+)$", text, re.M))

    def walk(refs):
        seen, found = set(), []
        pending = list(refs)
        while pending:
            key = pending.pop()
            if key in seen:
                continue
            need(key in nodes and len(seen) < 1024, "AIR metadata reference/bound")
            seen.add(key)
            value = nodes[key]
            found.append(value)
            pending.extend(re.findall(r"!(\d+)", value))
        return found

    def named(name):
        value = re.findall(r"^!" + re.escape(name) + r" = (.+)$", text, re.M)
        need(len(value) == 1, "AIR named metadata " + name)
        return re.findall(r"!(\d+)", value[0])

    options = "\n".join(walk(named("air.compile_options")))
    wanted = "disable" if mode == "precise" else "enable"
    opposite = "enable" if mode == "precise" else "disable"
    need('!"air.compile.denorms_disable"' in options, "denorm-disabled profile required")
    need(f'!"air.compile.fast_math_{wanted}"' in options and
         f'!"air.compile.fast_math_{opposite}"' not in options, "math profile changed")
    stages = named("air.fragment")
    need(len(stages) == 2, "two fragment entries required")
    for entry, scalar, vector in [("emit_f16", "half", "half4"), ("emit_f32", "float", "float4")]:
        roots = [n for n in stages if re.search(r"@" + entry + r"\b", nodes[n])]
        need(len(roots) == 1, "fragment metadata binding: " + entry)
        outputs = [n for n in walk(roots) if '!"air.render_target"' in n]
        need(len(outputs) == 1 and '!"air.render_target", i32 0, i32 0' in outputs[0] and
             f'!"{vector}"' in outputs[0], "color-zero return type: " + entry)
        bodies = re.findall(r"^define [^\n]*@" + entry + r"\(.*?^\}", text, re.M | re.S)
        need(len(bodies) == 1, "entry definition: " + entry)
        body = bodies[0]
        need(f"<4 x {scalar}>" in body.split("@", 1)[0] and
             re.search(r"\bret <4 x " + scalar + r">", body), "live return width")
        need("load " in body and "store " in body, "input load and return trace required")
        need(not re.search(r"\b(fadd|fsub|fmul|fdiv)\b|@air\.(fma|read_texture|sample_texture)", body),
             "shader-side blending or destination access forbidden")
        if scalar == "float":
            need("fptrunc" not in body and "<4 x half>" not in body, "float path narrowed")
    result = {"status": "PRELINK_PROFILE_BOUND", "mode": mode,
              "denorms": "disabled", "fast_math": mode == "fast", "return_types": ["half4", "float4"],
              "proof_scope": "pre-link AIR used by the recorded metallib command, not extracted linked AIR",
              "sha256": {n: digest(read(root / n)) for n in ("program.air", "program.ll", "program.metallib")}}
    save(root / "profile.json", result)


def validate(root, mode, write_outputs=True):
    corpus_data = read(root.parent / "cases.json")
    corpus = json.loads(corpus_data)
    cases = corpus["rows"]
    need(corpus["schema"] == 1 and len(cases) == 64 and len({c["id"] for c in cases}) == 64, "case set")
    source = read(root / "source.bin")
    seed = read(root / "seed.bin")
    need(len(source) == 1024 and len(seed) == 17408, "source/seed length")
    gpu = load(root / "gpu.json")
    prof = load(root / "profile.json")
    need(gpu["status"] == "NATIVE_BYTES_CAPTURED_NOT_MODEL_VALIDATION" and gpu["cpu_fallback"] is False,
         "native capture incomplete")
    need(gpu["mode"] == mode == prof["mode"] and gpu["device"] == "Apple Paravirtual device" and
         gpu["unified_memory"] is True, "native environment")
    need((gpu["runner"], gpu["os_build"]) in (("macos-26", "25G332"), ("xcode-27", "26A434")), "OS binding")
    need(re.fullmatch(r"[0-9a-f]{40}", gpu["reviewed_commit"]) is not None, "reviewed commit binding")
    need(gpu["loaded_sha256"] == prof["sha256"]["program.metallib"] == digest(read(root / "program.metallib")),
         "loaded library hash")
    for name in ("program.air", "program.ll"):
        need(prof["sha256"][name] == digest(read(root / name)), "compiler input changed")
    need(gpu["cases_sha256"] == digest(corpus_data) and gpu["source_sha256"] == digest(source) and
         gpu["seed_sha256"] == digest(seed), "input binding")
    need((gpu["command_buffers"], gpu["draws"], gpu["vertices"]) == (5, 4, 12), "fixed work bound")
    need(gpu["geometry"] == dict(rows=64, width=3, height=66, pitch=256, raw_guard_bytes=256,
        source_offset=64, trace_offset=64, trace_row_bytes=48, raw_bytes=17408, trace_bytes=3200), "geometry")
    records = gpu["captures"]
    need([(r["name"], r["return_bits"]) for r in records] == list(SPECS), "capture coverage/order")
    active = set()
    for i, case in enumerate(cases):
        need(len(case["source_f32"]) == len(case["destination_f16"]) == 4, "lane count")
        for lane in range(4):
            s, d = case["source_f32"][lane], case["destination_f16"][lane]
            need(re.fullmatch("[0-9a-f]{8}", s) and re.fullmatch("[0-9a-f]{4}", d), "hex input")
            need(struct.unpack_from("<I", source, i * 16 + lane * 4)[0] == int(s, 16), "source upload")
            at = 256 + (i + 1) * 256 + 8 + lane * 2
            need(struct.unpack_from("<H", seed, at)[0] == int(d, 16), "destination upload")
            active.update((at, at + 1))
    observations = ["mode\tcapture\treturn_bits\trow\tid\tlane\tsource_input_f32\treturned_bits\tdestination_f16\tnative_f16"]
    results, returned = {}, {}
    for index, record in enumerate(records):
        name, bits = SPECS[index]
        control = name.endswith("control")
        need(record["tag"] == (index + 1) | (256 if mode == "fast" else 0), "trace tag")
        need(record["negative_control"] is control and record["status"] == 4 and record["error"] is None and
             record["completed_within_10s"] is True and record["inputs_unchanged"] is True, "GPU command failure")
        need(record["texture"] == dict(format=115, storage=2, usage=5, width=3, height=66,
             samples=1, identity_swizzle=True), "texture metadata")
        need((record["draws"], record["vertices"]) == ((1, 3) if bits else (0, 0)), "draw count")
        raw = read(root / (name + ".raw.bin"))
        need(len(raw) == len(seed) and digest(raw) == record["raw_sha256"], "raw byte binding")
        if not bits:
            need(raw == seed, "native seed roundtrip")
            continue
        pd = record["pipeline_descriptor"]
        need(pd == dict(format=115, blend=[1, 5 if control else 3, 0, 1, 5, 0], blending_enabled=True,
             write_mask=15, fragment="emit_f16" if bits == 16 else "emit_f32", samples=1,
             alpha_to_coverage=False, alpha_to_one=False, rasterization=True), "pipeline descriptor binding")
        need(record["render_pass"] == dict(load=1, store=1, level=0, slice=0, depth_plane=0, resolve=False), "load/store state")
        need(all(a == b for i, (a, b) in enumerate(zip(raw, seed)) if i not in active), "untouched texels/row tails/raw guards")
        trace = read(root / (name + ".trace.bin"))
        need(len(trace) == 3200 and digest(trace) == record["trace_sha256"], "trace length/hash")
        need(trace[:64] == trace[-64:] == bytes([0xcd]) * 64, "trace guards")
        values, sources = [], []
        for i, case in enumerate(cases):
            marker = struct.unpack_from("<4I", trace, 64 + i * 48)
            need(marker == (0x52474241, i, bits, record["tag"]), "fragment coverage/identity")
            need(trace[80 + i * 48:96 + i * 48] == source[i * 16:(i + 1) * 16], "GPU input echo")
            sr = struct.unpack_from("<4I", trace, 96 + i * 48)
            output = struct.unpack_from("<4H", raw, 256 + (i + 1) * 256 + 8)
            if bits == 32:
                need(sr == struct.unpack_from("<4I", source, i * 16), "float return changed/narrowed")
            else:
                need(all(x <= 0xffff and (x & 0x7c00) != 0x7c00 for x in sr), "finite actual half return")
            need(all((x & 0x7c00) != 0x7c00 for x in output), "output outside this finite corpus scope")
            values.append(output); sources.append(sr)
            for lane in range(4):
                observations.append("\t".join([mode, name, str(bits), str(i), case["id"], "rgba"[lane],
                    case["source_f32"][lane], f"{sr[lane]:08x}", case["destination_f16"][lane], f"{output[lane]:04x}"]))
        results[name], returned[name] = values, sources
    for precision in ("half", "float"):
        a, b = results[precision + "-union"], results[precision + "-control"]
        need(returned[precision + "-union"] == returned[precision + "-control"], "control changed returned source")
        need(all(a[0][lane] != b[0][lane] for lane in range(3)), "wrong RGB factor negative control did not differ in all RGB lanes")
        need(all(x[3] == y[3] for x, y in zip(a, b)), "RGB-only control changed alpha")
        need(a[0][:3] == a[1][:3] == a[2][:3], "correct RGB depends on source/destination alpha")
        need(all(b[0][lane] != b[1][lane] for lane in range(3)), "source-alpha control insensitive")
        for row, changed in ((3, 0), (4, 1), (5, 2)):
            need(all(a[row][lane] == a[0][lane] for lane in range(4) if lane != changed), "cross-lane contamination")
        need(all(a[0][lane] != int(cases[0]["destination_f16"][lane], 16) for lane in range(4)), "all-channel write witness")
    text = "\n".join(observations) + "\n"
    validation = dict(status="NATIVE_INTEGRITY_AND_CONTROLS_PASS_NOT_MODEL_MATCH",
        mode=mode, native_lane_observations=1024, correct_blend_lanes=512, negative_control_lanes=512,
        observed_wrong_factor_differs=True, observed_alpha_unchanged=True, observed_lane_independence=True,
        private_comparison_required=True)
    if write_outputs:
        with (root / "observations.tsv").open("x", encoding="utf-8", newline="\n") as f:
            f.write(text)
        save(root / "validation.json", validation)
    return text, validation


def files(out):
    root = out / "results"
    need(root.is_dir() and not out.is_symlink() and not root.is_symlink(), "artifact root")
    found = []
    for p in root.rglob("*"):
        need(not p.is_symlink(), "artifact symlink")
        name = p.relative_to(root).as_posix()
        if p.is_dir():
            need(name in MODES, "artifact directory")
        else:
            need(name in ALLOWED, "unexpected artifact file: " + name)
            read(p)
            found.append(p)
    need(0 < len(found) <= 48 and sum(p.stat().st_size for p in found) <= 4194304, "artifact total bound")
    return sorted(found)


def manifest(out):
    return "".join(f"{digest(read(p))}  {p.relative_to(out).as_posix()}\n" for p in files(out))


def complete(root):
    for mode in MODES:
        need(load(root / mode / "validation.json")["status"] == "NATIVE_INTEGRITY_AND_CONTROLS_PASS_NOT_MODEL_MATCH",
             "both modes must pass")
    a, b = [load(root / m / "gpu.json") for m in MODES]
    for key in ("source_sha256", "seed_sha256", "cases_sha256", "device", "os_build", "runner", "reviewed_commit"):
        need(a[key] == b[key], "mode binding: " + key)
    need(a["loaded_sha256"] != b["loaded_sha256"], "precise and fast libraries unexpectedly identical")


def main():
    need(len(sys.argv) in (3, 4), "usage: audit.py profile|validate DIR MODE; complete RESULTS; seal|verify OUT")
    command, root = sys.argv[1], Path(sys.argv[2])
    if command in ("profile", "validate"):
        need(len(sys.argv) == 4 and sys.argv[3] in MODES, "mode")
        try:
            (profile if command == "profile" else validate)(root, sys.argv[3])
        except Exception as error:
            name = "profile.json" if command == "profile" else "validation.json"
            if not (root / name).exists():
                save(root / name, dict(status="UNQUALIFIED", failure=str(error), mode=sys.argv[3]))
            raise
    elif command == "complete":
        complete(root)
    elif command == "seal":
        text = manifest(root)
        with (root / "sha256.txt").open("x", encoding="utf-8", newline="\n") as f:
            f.write(text)
    elif command == "verify":
        need(read(root / "sha256.txt").decode("utf-8") == manifest(root), "artifact manifest mismatch")
    else:
        raise ValueError("unknown operation")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("UNQUALIFIED:", error, file=sys.stderr)
        sys.exit(3)
