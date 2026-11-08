#!/usr/bin/env python3
"""Synthetic inputs and independent integer/rational diagnostics; stdlib only.

The generate command writes INPUTS only. Native outputs are never synthesized,
repaired, narrowed, or substituted. Unit-test fixtures exist only in memory.
"""
import argparse
from bisect import bisect_right
import hashlib
import itertools
import json
from pathlib import Path
import struct
import unittest
from fractions import Fraction
from functools import lru_cache

W, H, IW, N = 17, 11, 15, 135
PITCH, GUARD, TG = 256, 256, 64
TARGET_BYTES, TRACE_BYTES = GUARD * 2 + H * PITCH, TG * 2 + N * 16
Q1_BYTES = TG * 2 + W * H * 24
MODES = {"shared": 0, "managed": 1, "private": 2}
OPS = ("read-f32", "read-f16", "sample-f32", "sample-f16")
MODELS = (
    ("f32-mul-rtz", "none", "f32", "rtz"),
    ("f32-mul-rne", "none", "f32", "rne"),
    ("source-half-rne-f32-rtz", "rne", "f32", "rtz"),
    ("source-half-rne-f32-rne", "rne", "f32", "rne"),
    ("source-half-rtz-f32-rtz", "rtz", "f32", "rtz"),
    ("source-half-rtz-f32-rne", "rtz", "f32", "rne"),
    ("exact-mul-rtz", "none", "exact", "rtz"),
    ("exact-mul-rne", "none", "exact", "rne"),
)


class Refusal(ValueError):
    pass


def require(ok, reason):
    if not ok:
        raise Refusal(reason)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encode_json(obj):
    return (json.dumps(obj, sort_keys=True, indent=2, allow_nan=False) + "\n").encode()


def pow2(e):
    return Fraction(1 << e, 1) if e >= 0 else Fraction(1, 1 << -e)


def fmt(precision):
    return (5, 10, 15) if precision == 16 else (8, 23, 127)


def finite(bits, precision):
    eb, fb, _ = fmt(precision)
    return ((bits >> fb) & ((1 << eb) - 1)) != (1 << eb) - 1


def decode(bits, precision):
    """Exact finite rational. Zero sign is carried separately by callers."""
    eb, fb, bias = fmt(precision)
    exp = (bits >> fb) & ((1 << eb) - 1)
    frac = bits & ((1 << fb) - 1)
    require(exp != (1 << eb) - 1, "decode-nonfinite")
    mag = Fraction(frac if exp == 0 else frac + (1 << fb)) * pow2((exp or 1) - bias - fb)
    return -mag if bits >> (precision - 1) else mag


def quantize(value, precision, rounding="rne", zero_sign=0):
    """Integer quotient/remainder binary conversion, including gradual underflow."""
    require(rounding in ("rne", "rtz"), "rounding")
    eb, fb, bias = fmt(precision)
    sign = int(value < 0) if value else zero_sign
    sign_bit = sign << (precision - 1)
    x = abs(Fraction(value))
    if not x:
        return sign_bit
    e = x.numerator.bit_length() - x.denominator.bit_length()
    if x < pow2(e):
        e -= 1
    inf = ((1 << eb) - 1) << fb
    if e > bias:
        return sign_bit | (inf if rounding == "rne" else inf - 1)
    scaled = x / pow2(max(e, 1 - bias) - fb)
    q, r = divmod(scaled.numerator, scaled.denominator)
    if rounding == "rne" and (r * 2 > scaled.denominator or (r * 2 == scaled.denominator and q & 1)):
        q += 1
    if e < 1 - bias:
        result = q
    else:
        result = ((e + bias) << fb) + q - (1 << fb)
    if result >= inf:
        result = inf if rounding == "rne" else inf - 1
    return sign_bit | result


def narrow(bits, precision, rounding):
    if finite(bits, precision):
        return quantize(decode(bits, precision), 16, rounding, bits >> (precision - 1))
    _, fb, _ = fmt(precision)
    return ((bits >> (precision - 1)) << 15) | (0x7e00 if bits & ((1 << fb) - 1) else 0x7c00)


@lru_cache(maxsize=32768)
def model(source, precision, dest, pre, accumulator, export):
    """A named candidate, NOT an assertion of fixed-function implementation."""
    if pre != "none":
        source, precision = narrow(source, precision, pre), 16
    sign = (source >> (precision - 1)) ^ (dest >> 15)
    for bits, p in ((source, precision), (dest, 16)):
        _, fb, _ = fmt(p)
        if not finite(bits, p) and bits & ((1 << fb) - 1):
            return None  # No invented NaN payload/sign policy.
    if not finite(source, precision) or not finite(dest, 16):
        other_zero = ((finite(source, precision) and decode(source, precision) == 0) or
                      (finite(dest, 16) and decode(dest, 16) == 0))
        return None if other_zero else (sign << 15) | 0x7c00
    exact = decode(source, precision) * decode(dest, 16)
    if accumulator == "f32":
        intermediate = quantize(exact, 32, "rne", sign)
        if not finite(intermediate, 32):
            return (sign << 15) | 0x7c00
        exact = decode(intermediate, 32)
    return quantize(exact, 16, export, sign)


def predictions(source, precision, dest):
    return [model(source, precision, dest, *m[1:]) for m in MODELS]


def fbits(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def hbits(value):
    return quantize(Fraction(value), 16)


def pattern(phase):
    b = bytearray((37 * x + 61 * y + 3 * x * y + 89 * phase) % 256 for y in range(H) for x in range(W))
    b[0], b[1] = ((0, 255), (255, 0), (1, 254))[phase]
    return bytes(b)


def backing(phase, row_pitch):
    require(256 <= row_pitch <= 4096 and row_pitch & (row_pitch - 1) == 0, "q1-pitch")
    d = bytearray([0xa7]) * (row_pitch * (H + 2))
    p = pattern(phase)
    for y in range(H):
        d[row_pitch * (y + 1): row_pitch * (y + 1) + W] = p[y * W: (y + 1) * W]
    return bytes(d)


def specs():
    out = []
    for precision in (32, 16):
        for name, mode, mask in (("mul-r", "mul", 8), ("mul-rg", "mul", 12),
                                 ("mul-none", "mul", 0), ("replace-rg", "replace", 12)):
            out.append(dict(name=f"f{precision}-{name}", precision=precision, mode=mode, mask=mask,
                            fragment="output_float" if precision == 32 else "output_half", negative=""))
    for negative, fragment, mode in (("wrong-kernel", "wrong_kernel", "mul"),
                                     ("wrong-output", "wrong_output", "mul"),
                                     ("wrong-state", "output_float", "replace"),
                                     ("wrong-mask", "output_float", "mul")):
        out.append(dict(name="negative-" + negative, precision=32, mode=mode, mask=12,
                        fragment=fragment, negative=negative))
    for i, s in enumerate(out, 1):
        s["id"] = i
    return out


def core_lane(source, precision, dest):
    if not finite(source, precision) or not finite(dest, 16):
        return False
    if not decode(source, precision) or not decode(dest, 16):
        return False
    p = predictions(source, precision, dest)
    return all(x is not None and 0 < (x & 0x7c00) < 0x7c00 for x in p)


@lru_cache(maxsize=1)
def vectors():
    out = []

    def add(kind, source, dest):
        out.append(dict(id=len(out), kind=kind, source=list(source), dest=list(dest)))

    for source, dest in (((.75, -1.5), (.5, 2)), ((-2, 3), (-4, .25)),
                         ((1, -1), (.75, -.5)), ((.5, .25), (4, -8))):
        add("exact-anchor", map(fbits, source), map(hbits, dest))
    for sign_s, sign_d in itertools.product((0, 1), repeat=2):
        add("signed-zero", (sign_s << 31, (sign_s ^ 1) << 31), (sign_d << 15, (sign_d ^ 1) << 15))
        add("zero-times-finite", (sign_s << 31, fbits(-2)), (hbits(-3), sign_d << 15))
    for src, dst in ((1, 0x7bff), (0x007fffff, 0x7bff), (0x33000000, 0x4000),
                     (0x33000001, 0x4000), (0x33800000, 0x3800), (0x33800001, 0x3800),
                     (fbits(.5), 1), (fbits(1.5), 1), (fbits(2), 0x03ff),
                     (fbits(65504), 3), (fbits(1.0009765625), 0x0400)):
        add("subnormal-underflow", (src, src | 0x80000000), (dst, dst))
    for src, dst in ((fbits(65504), hbits(2)), (fbits(65520), hbits(1)),
                     (fbits(65536), hbits(.5)), (fbits(1e10), hbits(1/1024)),
                     (0x7f7fffff, 0x7bff), (fbits(32760), hbits(2))):
        add("finite-overflow", (src, src | 0x80000000), (dst, dst))
    for value in (0x7c01, 0xfc03, 0x8000, 0x0001):
        add("masked-g-sentinel", (fbits(.75), fbits(.5)), (hbits(.5), value))

    # Independently constructed products near half boundaries/midpoints. Division
    # selects INPUT bits only; exact rational products then identify discriminators.
    pairs = set(itertools.combinations(range(len(MODELS)), 2))

    def coverage(src, dst):
        if not core_lane(src, 32, dst):
            return set()
        p = predictions(src, 32, dst)
        return {(a, b) for a, b in pairs if p[a] != p[b]}

    covered = set()
    for row in out:
        for src, dst in zip(row["source"], row["dest"]):
            covered |= coverage(src, dst)
    candidates = []
    for h in (0x0401, 0x3555, 0x3801, 0x3bff, 0x3c00, 0x4001, 0x6401, 0x7bfe):
        for dest in (0x3555, 0x3556, 0x3aab, 0x3c01, 0x3d55, 0x4001, 0x5403, 0x0401, 0x0003):
            for target in (decode(h, 16), (decode(h, 16) + decode(h + 1, 16)) / 2):
                center = quantize(target / decode(dest, 16), 32)
                for delta in (-2, -1, 0, 1, 2):
                    src = center + delta
                    if finite(src, 32):
                        candidates.append((src, dest, coverage(src, dest)))
    while covered != pairs:
        best = max(candidates, key=lambda c: len(c[2] - covered))
        require(bool(best[2] - covered), "cannot-separate-enumerated-f32-models")
        src, dst, seen = best
        add("rounding-discriminator", (src, src | 0x80000000), (dst, dst))
        covered |= seen
    # Fill a bounded 15x9 interior with reproducible, unrelated finite pairs.
    state = 0x1a2b3c4d
    while len(out) < N:
        state = (1664525 * state + 1013904223) & 0xffffffff
        src = 0x3e800000 + (state & 0x01ffffff)
        dest = 0x3000 + ((state >> 7) % 0x1800)
        add("finite-grid", (src, (src ^ 0x80000000) + 3), (dest, dest ^ 0x8000))
    require(len(out) == N, "vector-budget")
    return out


def word_offset(i, lane):
    y, x = divmod(i, IW)
    return GUARD + (y + 1) * PITCH + (x + 1) * 4 + lane * 2


def u16(b, at):
    return struct.unpack_from("<H", b, at)[0]


def put16(b, at, value):
    struct.pack_into("<H", b, at, value)


def seed():
    d = bytearray([0xcd]) * TARGET_BYTES
    border = (0x8000, 1, 0x7c01, 0xfc02, 0x3555, 0x0401)
    for y in range(H):
        for x in range(W):
            for lane in range(2):
                value = border[(x * 7 + y * 5 + lane) % len(border)]
                if 1 <= x <= IW and 1 <= y <= 9:
                    value = vectors()[(y - 1) * IW + x - 1]["dest"][lane]
                put16(d, GUARD + y * PITCH + x * 4 + lane * 2, value)
    return bytes(d)


def design_classes(precision):
    signatures = [[] for _ in MODELS]
    for row in vectors():
        for source, dest in zip(row["source"], row["dest"]):
            src = source if precision == 32 else narrow(source, 32, "rne")
            if core_lane(src, precision, dest):
                for sig, p in zip(signatures, predictions(src, precision, dest)):
                    sig.append(p)
    groups = {}
    for m, sig in zip(MODELS, signatures):
        groups.setdefault(tuple(sig), []).append(m[0])
    return list(groups.values())


def inputs():
    manifest = dict(schema=1, width=W, height=H, pitch=PITCH, guardBytes=GUARD,
                    traceGuard=TG, rowCount=N, q2=specs(), vectors=vectors(),
                    q1=dict(modes=list(MODES), phases=["initial", "cpu-write-or-private-upload", "gpu-copy-mutation", "owner-release"],
                            parentUsage=23, directControlUsage=1, directControlLegality="independent native API unknown",
                            operations=list(OPS), valueDispatchesPerSupportedMode=63, recordsPerSupportedMode=69,
                            one_sided_mutation_controls="updated view versus unchanged independent direct A8 before updating the reference"),
                    models=[m[0] for m in MODELS],
                    design_equivalence_classes={"float2": design_classes(32), "half2_assuming_rne_source": design_classes(16)},
                    arithmetic_scope="enumerated models only; half-product intermediate precision may be observationally equivalent")
    return {"cases.json": encode_json(manifest), "q1-patterns.bin": b"".join(pattern(i) for i in range(3)),
            "q2-source.bin": b"".join(struct.pack("<II", *r["source"]) for r in vectors()), "q2-seed.bin": seed()}


def generate(out):
    out = Path(out)
    require(out.is_dir() and not out.is_symlink(), "generation-directory")
    blobs = inputs()
    require(all(not (out / name).exists() for name in blobs), "generation-reuse")
    for name, data in blobs.items():
        with (out / name).open("xb") as f:
            f.write(data)
    return {name: dict(bytes=len(data), sha256=digest(data)) for name, data in blobs.items()}


def load_capture(out, label):
    report = json.loads((out / (label + ".json")).read_text())
    require(report.get("status") == "CAPTURED_PENDING_INDEPENDENT_VALIDATION" and report.get("exit_code") == 0,
            "native-not-complete:" + label)
    require(report.get("probe") == label and report.get("cpu_result_fallback") is False and
            report.get("resource_substitution") is False and report.get("source_compile_fallback") is False, "native-provenance")
    require(report.get("device", {}).get("name") and report.get("device", {}).get("registry_id") and
            report.get("device", {}).get("architecture") and report.get("os"), "device-os-missing")
    metadata = json.loads((out / "metadata.json").read_text())
    require(report["os_build_from_runner_sw_vers"] == metadata["actual_os_build"], "native-os-build-binding")
    require(metadata["actual_os_build"] in ("25G332", "26A434") and
            metadata["actual_os_build"] == metadata["expected_os_build"] and metadata["native_execution_attempted"] is True,
            "native-reference-build-or-execution-metadata")
    require(set(metadata.get("tool_binaries", {})) == {"metal", "metallib", "swiftc"} and
            all(len(t.get("sha256", "")) == 64 for t in metadata["tool_binaries"].values()), "compiler-binary-pins")
    pair = json.loads((out / "compile-pair.json").read_text())
    source_pins = json.loads((out / "source-sha256.json").read_text())
    shader_pins = [p for p in source_pins if p["name"] == "scripts/native-bounded/probe.metal"]
    require(len(shader_pins) == 1 and pair["source_sha256"] == shader_pins[0]["sha256"] and
            pair["air_sha256"] == digest((out / "probe.air").read_bytes()) and
            pair["metallib_sha256"] == digest((out / "probe.metallib").read_bytes()) and
            pair["compile_exit"] == 0 and pair["link_exit"] == 0 and pair["source_compile_fallback"] is False and
            pair["link_input"] == "probe.air" and pair["language"] == "metal3.1", "offline-input-AIR-library-pins")
    require(pair["compile_argv"] == ["xcrun", "--sdk", "macosx", "metal", "-std=metal3.1", "-O2", "-g0", "-fno-fast-math",
            "-c", "scripts/native-bounded/probe.metal", "-o", "out-native-bounded/probe.air"] and
            pair["link_argv"] == ["xcrun", "--sdk", "macosx", "metallib", "out-native-bounded/probe.air", "-o", "out-native-bounded/probe.metallib"],
            "offline-compiler-command-contract")
    require(report.get("compile_pair") == pair and report.get("loaded_metallib_sha256") == digest((out / "probe.metallib").read_bytes()),
            "loaded-library-pair")
    require(report["input_manifest_sha256"] == digest(inputs()["cases.json"]), "native-input-manifest")
    commands = report.get("commands", [])
    require(commands and [c["id"] for c in commands] == list(range(len(commands))), "command-id-sequence")
    require(all(c["status"] == 4 and c["completed"] is True and c["error_present"] is False for c in commands), "gpu-command-incomplete")
    require(len({c["name"] for c in commands}) == len(commands), "duplicate-command")
    blob = (out / (label + ".gpu.bin")).read_bytes()
    require(len(blob) == report["raw_bytes"] <= 1_048_576 and digest(blob) == report["raw_sha256"], "raw-file-pin")
    records, offset = [], 0
    for r in report["records"]:
        require(r["offset"] == offset and 0 < r["length"] <= 1_048_576, "raw-span-sequence")
        data = blob[offset: offset + r["length"]]
        require(len(data) == r["length"] and digest(data) == r["sha256"], "raw-span-pin")
        records.append((r, data)); offset += len(data)
    require(offset == len(blob), "raw-trailing-data")
    return report, records


def texture_contract(t, fmt_num, storage, buffer_backed=None):
    expected = dict(format=fmt_num, storage=storage, type=2, width=W, height=H, depth=1, mips=1, slices=1, samples=1)
    require(all(t.get(k) == v for k, v in expected.items()), "actual-texture-contract")
    require(t.get("framebuffer_only") is False, "framebuffer-only")
    if buffer_backed is not None:
        require(t.get("buffer_backed_property") == buffer_backed, "backing-class")


def q1_values(r, data):
    require(len(data) == Q1_BYTES, "q1-value-length")
    require(data[:TG] == bytes([0xcd]) * TG and data[-TG:] == bytes([0xcd]) * TG, "q1-output-guard")
    result = []
    for i in range(W * H):
        marker, index, *bits = struct.unpack_from("<6I", data, TG + i * 24)
        require(marker == 0x51710000 ^ r["tag"] and index == i, "q1-marker")
        if r["precision"] == 16:
            require(all(x <= 65535 for x in bits), "q1-half-word-width")
        result.append(tuple(bits))
    return result


def q1_semantics(values, phase, operation, precision, is_r8=False):
    p = pattern(min(phase, 2))
    one = 0x3f800000 if precision == 32 else 0x3c00
    tolerance = Fraction(1, 1_000_000) if precision == 32 else Fraction(1, 2000)
    for i, b in enumerate(values):
        y, x = divmod(i, W)
        x = min(x + 1, W - 1) if operation.startswith("sample") else x
        value = p[y * W + x]
        lane = 0 if is_r8 else 3
        require((b[1], b[2]) == (0, 0) and b[3 if is_r8 else 0] == (one if is_r8 else 0), "q1-channels")
        require(finite(b[lane], precision) and abs(decode(b[lane], precision) - Fraction(value, 255)) <= tolerance, "q1-unorm")
        if value in (0, 255):
            require(b[lane] == (0 if value == 0 else one), "q1-endpoint")


def validate_q1(out, mode):
    report, rows = load_capture(out, "q1-" + mode)
    require(len(rows) == 69 and report.get("value_dispatches") == 63, "q1-coverage")
    requested = report["requested"]
    require(requested["storage"] == MODES[mode] and requested["swizzle"] == [2, 3, 4, 5], "q1-requested-mode-swizzle")
    require(requested["parent_usage_raw"] == 23 and requested["direct_control_usage_raw"] == 1, "q1-requested-usage23")
    require(report.get("comparison_ready") is True and
            all(report["creation_outcomes"][role]["status"] == "CREATED_ACTUAL_PROPERTIES_VERIFIED"
                for role in ("candidate", "direct_control")), "q1-independent-creation-not-qualified")
    row_pitch = requested["pitch"]
    require(requested["offset"] == row_pitch and requested["length"] == len(backing(0, row_pitch)), "q1-buffer-layout")
    require(report["input_patterns_sha256"] == digest(b"".join(pattern(i) for i in range(3))), "q1-input-pin")
    require(len(report["samplers_at_creation"]) == 2, "q1-sampler-count")
    for index, sampler in enumerate(report["samplers_at_creation"]):
        require(sampler == dict(linear=bool(index), min=index, mag=index, mip=0, normalized=True,
                address_str=[0, 0, 0], anisotropy=1, lod_min=0, lod_max=0), "q1-sampler-state")
    for name, form in (("parent_r8", 10), ("direct_a8", 1)):
        texture_contract(report[name], form, MODES[mode], True)
        require(report[name]["usage"] == (23 if name == "parent_r8" else 1) and
                report[name]["swizzle"] == [2, 3, 4, 5], "q1-base-usage-swizzle")
        require(report[name]["buffer_offset"] == row_pitch and report[name]["buffer_pitch"] == row_pitch, "q1-base-buffer-layout")
        require(report[name]["backing_buffer_storage"] == MODES[mode] and
                report[name]["backing_buffer_length"] == requested["length"], "q1-backing-buffer-storage")
    for name in ("view_a8", "sibling_a8"):
        t = report[name]
        texture_contract(t, 1, MODES[mode])
        require(t["usage"] == 23 and t["parent_identity_matches"] and t["backing_identity_matches"] and t["parent_present"] and
                t["parent_level"] == 0 and t["parent_slice"] == 0 and t["swizzle"] == [2, 3, 4, 5], "q1-view-alias-identity")
    require(all(report["lifetime"].get(k) is True for k in ("explicit_original_owners_released", "autorelease_pool_drained",
            "prior_commands_completed", "only_surviving_view_and_independent_control_escaped")), "q1-lifetime")
    positives, negatives, backing_seen, tags = {}, {}, set(), []
    for r, data in rows:
        if r["kind"] == "backing":
            key = (r["phase"], r["role"])
            require(key not in backing_seen and r["pitch"] == row_pitch and r["buffer_offset"] == row_pitch, "q1-backing-key")
            require(r["name"] == f"p{r['phase']}-backing-{r['role']}", "q1-backing-name")
            require(data == backing(r["phase"], row_pitch), "q1-backing-or-padding")
            backing_seen.add(key); continue
        require(r["kind"] == "values" and r["operation"] in OPS and r["precision"] == (32 if r["operation"].endswith("32") else 16), "q1-operation")
        vals = q1_values(r, data); tags.append(r["tag"])
        expected_name = f"p{r['phase']}-{r['role']}-{r['operation']}" + ("-" + r["negative"] if r["negative"] else "")
        require(r["name"] == expected_name, "q1-dispatch-name")
        form = 10 if r["role"] == "r8-negative" or r["negative"] == "wrong-output" else 1
        texture_contract(r["actual_texture"], form, MODES[mode])
        require(r["actual_texture"]["usage"] == (1 if r["role"] in ("direct", "old-direct") else 23), "q1-bound-texture-usage")
        if r["negative"]:
            require(r["negative"] not in negatives and r["phase"] == 0 and r["role"] == "view", "q1-negative-identity")
            negatives[r["negative"]] = (r, vals); continue
        key = (r["phase"], r["role"], r["operation"])
        require(key not in positives and r["sampler"] == "nearest", "q1-positive-identity")
        expected_kernel = r["operation"].replace("-f32", "_float").replace("-f16", "_half")
        require(r["kernel"] == expected_kernel, "q1-positive-kernel")
        semantic_phase = r["phase"] - 1 if r["role"] == "old-direct" else r["phase"]
        q1_semantics(vals, semantic_phase, r["operation"], r["precision"], r["role"] == "r8-negative")
        positives[key] = vals
    expected = {(p, role, op) for p in range(3) for role in ("view", "sibling", "direct", "r8-negative") for op in OPS}
    expected |= {(3, role, op) for role in ("view", "direct") for op in OPS}
    expected |= {(p, role, "read-f32") for p in (1, 2) for role in ("updated-only-view", "old-direct")}
    require(set(positives) == expected and tags == list(range(1, 64)), "q1-complete-matrix")
    require(backing_seen == set(itertools.product(range(3), ("r8", "a8"))), "q1-all-backing-readbacks")
    command_names = []
    for phase in range(3):
        if phase == 0:
            command_names.append(f"p{phase}-upload")
        else:
            if phase != 1 or mode == "private":
                command_names.append(f"p{phase}-r-upload")
            command_names += [f"p{phase}-{role}-read-f32" for role in ("updated-only-view", "old-direct")]
            if phase != 1 or mode == "private":
                command_names.append(f"p{phase}-a-upload")
        command_names += [f"p{phase}-{role}-{op}" for role in ("view", "sibling", "direct", "r8-negative") for op in OPS]
        if phase == 0:
            command_names += ["p0-view-read-f32-wrong-kernel", "p0-view-read-f32-wrong-output", "p0-view-sample-f32-wrong-state"]
        command_names.append(f"p{phase}-backing-readback")
    command_names += [f"p3-{role}-{op}" for role in ("view", "direct") for op in OPS]
    require([c["name"] for c in report["commands"]] == command_names, "q1-command-plan")
    for phase in range(3):
        for op in OPS:
            candidate = positives[phase, "view", op]
            require(candidate == positives[phase, "direct", op] == positives[phase, "sibling", op], "q1-view-direct-alias-disagree")
            require(candidate != positives[phase, "r8-negative", op], "q1-r8-negative-not-distinct")
            if phase:
                require(candidate != positives[phase - 1, "view", op], "q1-mutation-invisible")
    for op in OPS:
        require(positives[3, "view", op] == positives[3, "direct", op] == positives[2, "view", op], "q1-lifetime-value-change")
    for phase in (1, 2):
        changed = positives[phase, "updated-only-view", "read-f32"]
        unchanged = positives[phase, "old-direct", "read-f32"]
        require(changed == positives[phase, "view", "read-f32"] and
                unchanged == positives[phase-1, "direct", "read-f32"] and changed != unchanged,
                "q1-one-sided-mutation-or-independent-reference")
    require(set(negatives) == {"wrong-kernel", "wrong-output", "wrong-state"}, "q1-negative-coverage")
    negative_receipts = {}
    for name, (r, vals) in negatives.items():
        op, kernel, sampler = {"wrong-kernel": ("read-f32", "wrong_read_float", "nearest"),
            "wrong-output": ("read-f32", "read_float", "nearest"),
            "wrong-state": ("sample-f32", "sample_float", "linear")}[name]
        require((r["operation"], r["kernel"], r["sampler"]) == (op, kernel, sampler), "q1-negative-native-state")
        require(vals != positives[0, "view", r["operation"]], "q1-negative-not-observable")
        if name != "wrong-state":
            try:
                q1_semantics(vals, 0, r["operation"], r["precision"])
            except Refusal as e:
                negative_receipts[name] = str(e)
            else:
                raise Refusal("q1-negative-passed-ordinary-check")
        else:
            require(r["sampler"] == "linear" and r["operation"] == "sample-f32", "q1-wrong-state-not-linear")
            negative_receipts[name] = "different actual GPU sample vector under linear sampler"
    return dict(status="VALIDATED_NATIVE_COMPARISONS", mode=mode, value_dispatches=63,
                exact_view_direct_alias=True, mutations_observed=True, independent_reference_observed=True,
                owner_release_observed=True, negatives=negative_receipts)


def trace_values(data, spec):
    require(len(data) == TRACE_BYTES, "trace-length")
    require(data[:TG] == bytes([0xcd]) * TG and data[-TG:] == bytes([0xcd]) * TG, "trace-guard")
    values = []
    for i in range(N):
        marker, index, r, g = struct.unpack_from("<4I", data, TG + i * 16)
        require(marker == (0x51720000 ^ spec["id"]) and index == i, "trace-marker")
        require(spec["precision"] == 32 or (r <= 65535 and g <= 65535), "trace-half-width")
        values.append((r, g))
    return values


def preservation(target, mask):
    require(len(target) == TARGET_BYTES, "target-length")
    enabled = set()
    for i in range(N):
        for lane, bit in enumerate((8, 4)):
            if mask & bit:
                at = word_offset(i, lane)
                enabled.update((at, at + 1))
    original = seed()
    require(all(target[i] == original[i] for i in range(len(target)) if i not in enabled), "preservation-G-border-or-padding")


def ordinary_q2(target, trace, spec, expected_mask=None, expected_mode=None):
    """Same positive check used to reject actual negative GPU captures."""
    mask = spec["mask"] if expected_mask is None else expected_mask
    mode = spec["mode"] if expected_mode is None else expected_mode
    preservation(target, mask)
    values = trace_values(trace, spec)
    for i, (row, actual) in enumerate(zip(vectors(), values)):
        expected = row["source"] if spec["precision"] == 32 else [narrow(v, 32, "rne") for v in row["source"]]
        # Float pass-through must be exact; half inexact conversion is measured,
        # not silently repaired. Exact half-representable anchors remain strict.
        if spec["precision"] == 32 or row["kind"] == "exact-anchor":
            require(tuple(expected) == actual, "return-source")
        if row["kind"] == "exact-anchor":
            for lane, bit in enumerate((8, 4)):
                if not mask & bit:
                    continue
                x = decode(actual[lane], spec["precision"])
                if mode == "mul":
                    x *= decode(row["dest"][lane], 16)
                require(u16(target, word_offset(i, lane)) == quantize(x, 16), "exact-anchor-output")
    return values


def state_contract(r, spec):
    expected = dict(format=65, mask=spec["mask"], blending=True, rgb_op=0, alpha_op=0,
                    src_rgb=6 if spec["mode"] == "mul" else 1, dst_rgb=0, src_alpha=8, dst_alpha=0,
                    samples=1, fragment=spec["fragment"], vertex="full_triangle", scissor=[1, 1, IW, 9],
                    viewport=[0, 0, W, H, 0, 1], load="load", store="store", depth_stencil="none", cull="none")
    require(all(r["state"].get(k) == v for k, v in expected.items()), "pipeline-state-receipt")
    require(all(r[k] == spec[k] for k in ("name", "precision", "mode", "mask", "negative")) and r["tag"] == spec["id"], "q2-record-identity")
    texture_contract(r["actual_texture"], 65, 2, False)
    require(r["actual_texture"]["usage"] == 4, "q2-render-target-usage")


def analyze_models(target, trace, spec):
    values = trace_values(trace, spec)
    signatures = [[] for _ in MODELS]
    actual_core = []
    statistics = {m[0]: {} for m in MODELS}
    lines = []
    half_conversion_mismatches = 0
    for i, row in enumerate(vectors()):
        for lane in range(2):
            source, dest = values[i][lane], row["dest"][lane]
            actual = u16(target, word_offset(i, lane))
            if spec["precision"] == 16 and source != narrow(row["source"][lane], 32, "rne"):
                half_conversion_mismatches += 1
            p = predictions(source, spec["precision"], dest)
            core = core_lane(source, spec["precision"], dest)
            if core:
                actual_core.append(actual)
                for sig, value in zip(signatures, p):
                    sig.append(value)
            for m, value in zip(MODELS, p):
                bucket = statistics[m[0]].setdefault(row["kind"], dict(compared=0, mismatches=0, unmodeled=0))
                if value is None:
                    bucket["unmodeled"] += 1
                else:
                    bucket["compared"] += 1
                    bucket["mismatches"] += int(value != actual)
            lines.append("\t".join([str(spec["precision"]), str(i), "RG"[lane], row["kind"],
                f"{row['source'][lane]:08x}", f"{source:08x}", f"{dest:04x}", f"{actual:04x}", str(int(core))] +
                ["unmodeled" if v is None else f"{v:04x}" for v in p]))
    groups = {}
    for m, sig in zip(MODELS, signatures):
        groups.setdefault(tuple(sig), []).append(m[0])
    classes = [dict(models=names, core_mismatches=sum(a != b for a, b in zip(sig, actual_core)))
               for sig, names in groups.items()]
    winners = [g["models"] for g in classes if g["core_mismatches"] == 0]
    require(len(actual_core) >= 128, "too-few-model-core-words")
    return dict(core_words=len(actual_core), core_equivalence_classes=classes, matching_classes=winners,
                resolved_among_enumerated_classes=len(winners) == 1, by_input_category=statistics,
                half_source_rne_mismatches=half_conversion_mismatches if spec["precision"] == 16 else None,
                zero_subnormal_overflow_words_retained=True,
                scope="core excludes zeros, subnormal/overflow model outputs and NaNs; those remain in the diagnostic table",
                physical_intermediate_width_identified=False), lines


def validate_q2(out):
    report, records = load_capture(out, "q2")
    require(len(records) == 25 and len(report["commands"]) == 13 and report.get("input_buffers_unchanged") is True, "q2-coverage")
    original = inputs()
    require(report["input_sha256"] == {"source": digest(original["q2-source.bin"]), "seed": digest(original["q2-seed.bin"])}, "q2-input-pins")
    baseline = [(r, d) for r, d in records if r["name"] == "baseline"]
    require(len(baseline) == 1 and baseline[0][0]["kind"] == "target" and baseline[0][1] == seed(), "baseline-roundtrip")
    texture_contract(baseline[0][0]["actual_texture"], 65, 2, False)
    by_key = {}
    expected_specs = specs()
    for r, data in records:
        key = (r["name"], r["kind"])
        require(key not in by_key, "duplicate-q2-record")
        by_key[key] = data
        if r["name"] != "baseline":
            matching = [s for s in expected_specs if s["name"] == r["name"]]
            require(len(matching) == 1 and r["kind"] in ("target", "trace"), "unknown-q2-record")
            state_contract(r, matching[0])
    required = {(s["name"], k) for s in expected_specs for k in ("target", "trace")} | {("baseline", "target")}
    require(set(by_key) == required, "q2-missing-output")
    require([c["name"] for c in report["commands"]] == ["baseline"] + [s["name"] for s in expected_specs], "q2-draw-command-plan")
    negatives = {}
    for spec in expected_specs:
        target, trace = (by_key[spec["name"], k] for k in ("target", "trace"))
        if not spec["negative"]:
            ordinary_q2(target, trace, spec)
            continue
        preservation(target, spec["mask"])
        trace_values(trace, spec)
        expected_mask = 8 if spec["negative"] == "wrong-mask" else 12
        reference = by_key["f32-mul-r" if expected_mask == 8 else "f32-mul-rg", "target"]
        require(target != reference, "negative-GPU-output-not-distinct")
        try:
            ordinary_q2(target, trace, spec, expected_mask=expected_mask, expected_mode="mul")
        except Refusal as e:
            negatives[spec["negative"]] = str(e)
        else:
            raise Refusal("negative-passed-positive-validator")
    require(set(negatives) == {"wrong-kernel", "wrong-output", "wrong-state", "wrong-mask"}, "q2-negative-coverage")
    for precision in (32, 16):
        r, rg = (by_key[f"f{precision}-mul-{mask}", "target"] for mask in ("r", "rg"))
        require(all(u16(r, word_offset(i, 0)) == u16(rg, word_offset(i, 0)) for i in range(N)), "red-R-vs-RG-mismatch")
        returned = []
        for spec in expected_specs:
            if spec["precision"] == precision and not spec["negative"]:
                returned.append(trace_values(by_key[spec["name"], "trace"], spec))
        require(all(v == returned[0] for v in returned), "return-trace-state-dependent")
    model_reports, lines = {}, []
    for precision in (32, 16):
        spec = next(s for s in expected_specs if s["name"] == f"f{precision}-mul-rg")
        summary, table = analyze_models(by_key[spec["name"], "target"], by_key[spec["name"], "trace"], spec)
        model_reports[str(precision)] = summary; lines += table
    header = ["precision", "vector", "lane", "kind", "input_f32", "native_return_bits", "dest_f16", "gpu_result_f16", "core"]
    table = "\t".join(header + [m[0] for m in MODELS]) + "\n" + "\n".join(lines) + "\n"
    with (out / "q2-models.tsv").open("xb") as f:
        f.write(table.encode())
    resolved = all(r["resolved_among_enumerated_classes"] for r in model_reports.values())
    return dict(status="VALIDATED_NATIVE_MODELS_IDENTIFIED" if resolved else "VALIDATED_CAPTURE_MODEL_UNRESOLVED",
                captures=13, negatives=negatives, masked_green_border_and_blit_padding_exact=True,
                models=model_reports, half_product_precision_note="For half operands the exact product fits binary32; equivalent models cannot identify physical hardware precision.",
                alpha_note="The alpha factors are recorded but RG16Float has no stored B/A; no direct B/A shader access is made. Hidden hardware accesses are not inferred.")


def validate(out):
    out = Path(out)
    require(not (out / "validation.json").exists() and not (out / "q2-models.tsv").exists(), "validation-reuse")
    for name, data in inputs().items():
        require((out / name).read_bytes() == data, "deterministic-input-pin:" + name)
    result = dict(schema=1, cpu_result_fallback=False, status="NOT_FULLY_QUALIFIED", probes={})
    for label in ["q1-" + m for m in MODES] + ["q2"]:
        try:
            r = validate_q2(out) if label == "q2" else validate_q1(out, label[3:])
        except (Refusal, OSError, KeyError, TypeError, ValueError) as e:
            # No arbitrary native errors, environment dumps or filenames outside output.
            r = dict(status="NOT_QUALIFIED", reason=str(e) if isinstance(e, Refusal) else type(e).__name__)
            if label.startswith("q1-"):
                try:
                    native = json.loads((out / (label + ".json")).read_text())
                    if "creation_outcomes" in native:
                        r["creation_outcomes"] = native["creation_outcomes"]
                        r["creation_scope"] = "Separate candidate/control checkpoint outcomes; ATTEMPTING after process failure is incomplete, never a success."
                except (OSError, ValueError, TypeError):
                    pass  # No native receipt exists; do not invent creation results.
        result["probes"][label] = r
    devices = {}
    for label, probe in result["probes"].items():
        if probe["status"].startswith("VALIDATED_"):
            device = json.loads((out / (label + ".json")).read_text())["device"]
            devices[label] = {k: device[k] for k in ("name", "registry_id", "architecture")}
    result["devices_of_validated_captures"] = devices
    result["same_device_across_validated_captures"] = bool(devices) and len({tuple(d.values()) for d in devices.values()}) == 1
    if (all(result["probes"]["q1-" + m]["status"] == "VALIDATED_NATIVE_COMPARISONS" for m in MODES) and
            result["probes"]["q2"]["status"] == "VALIDATED_NATIVE_MODELS_IDENTIFIED" and
            result["same_device_across_validated_captures"]):
        result["status"] = "BOUNDED_NATIVE_OBSERVATIONS_VALIDATED"
    result["unsupported_policy"] = "Nil, unsupported, remapped, incomplete, or unresolved lanes are NOT passes; no mode/class substitution. Successful lanes remain separately labeled."
    with (out / "validation.json").open("xb") as f:
        f.write(encode_json(result))
    return result


def _fixture(spec):
    """UNIT TEST ONLY, memory-only arrays. Never used by generate/validate/native."""
    target, trace = bytearray(seed()), bytearray([0xcd]) * TRACE_BYTES
    for i, row in enumerate(vectors()):
        emitted = list(row["source"])
        if spec["negative"] == "wrong-kernel":
            emitted.reverse()
        if spec["precision"] == 16:
            emitted = [narrow(v, 32, "rne") for v in emitted]
        struct.pack_into("<4I", trace, TG + i * 16, 0x51720000 ^ spec["id"], i, *emitted)
        for lane, bit in enumerate((8, 4)):
            if not spec["mask"] & bit:
                continue
            if spec["negative"] == "wrong-output":
                value = 0
            elif spec["mode"] == "replace":
                value = narrow(emitted[lane], spec["precision"], "rtz" if spec["precision"] == 32 else "rne")
            else:
                value = model(emitted[lane], spec["precision"], row["dest"][lane], "none", "f32", "rtz" if spec["precision"] == 32 else "rne")
            put16(target, word_offset(i, lane), 0x7e00 if value is None else value)
    return target, trace


class Tests(unittest.TestCase):
    def test_half_roundtrip_all_finite_encodings(self):
        for h in range(65536):
            if finite(h, 16):
                independent = Fraction.from_float(struct.unpack("<e", struct.pack("<H", h))[0])
                self.assertEqual(decode(h, 16), independent)
                self.assertEqual(quantize(decode(h, 16), 16, "rne", h >> 15), h)
                self.assertEqual(quantize(decode(h, 16), 16, "rtz", h >> 15), h)

    def test_rtz_against_ordered_independent_half_set(self):
        table = [Fraction.from_float(struct.unpack("<e", struct.pack("<H", h))[0]) for h in range(0x7c00)]
        for row in vectors():
            for source, dest in zip(row["source"], row["dest"]):
                if not finite(dest, 16):
                    continue
                for value in (decode(source, 32), decode(source, 32) * decode(dest, 16)):
                    expected = (int(value < 0) << 15) | (bisect_right(table, abs(value)) - 1)
                    self.assertEqual(quantize(value, 16, "rtz"), expected)

    def test_binary32_product_rounding_against_independent_struct(self):
        # A binary32 x binary16 finite product has at most 35 significant bits,
        # so Python's binary64 product is exact before struct's binary32 rounding.
        for row in vectors():
            for source, dest in zip(row["source"], row["dest"]):
                if not finite(dest, 16):
                    continue
                a = struct.unpack("<f", struct.pack("<I", source))[0]
                b = struct.unpack("<e", struct.pack("<H", dest))[0]
                sign = (source >> 31) ^ (dest >> 15)
                try:
                    expected = struct.unpack("<I", struct.pack("<f", a * b))[0]
                except OverflowError:
                    expected = (sign << 31) | 0x7f800000
                self.assertEqual(quantize(decode(source, 32) * decode(dest, 16), 32, "rne", sign), expected)

    def test_rne_against_independent_struct(self):
        for h in (0, 1, 0x03ff, 0x0400, 0x3555, 0x3bff, 0x3c00, 0x7bfe):
            midpoint = (decode(h, 16) + decode(h + 1, 16)) / 2
            b = quantize(midpoint, 32)
            for source in (b - 1 if b else 0, b, b + 1):
                value = float(decode(source, 32))
                for sign in (1, -1):
                    expected = struct.unpack("<H", struct.pack("<e", sign * value))[0]
                    self.assertEqual(quantize(Fraction(sign * value), 16, zero_sign=int(sign < 0)), expected)

    def test_overflow_underflow_and_zero_sign(self):
        self.assertEqual(quantize(Fraction(65520), 16), 0x7c00)
        self.assertEqual(quantize(Fraction(-65520), 16, "rtz"), 0xfbff)
        self.assertEqual(quantize(pow2(-25), 16), 0)
        self.assertEqual(quantize(pow2(-25) + pow2(-48), 16), 1)
        self.assertEqual(quantize(-pow2(-25), 16), 0x8000)
        self.assertEqual(quantize(Fraction(0), 16, zero_sign=1), 0x8000)

    def test_f32_rounding_ties(self):
        self.assertEqual(quantize(Fraction(1) + pow2(-24), 32), 0x3f800000)
        self.assertEqual(quantize(Fraction(1) + 3 * pow2(-24), 32), 0x3f800002)
        self.assertEqual(quantize(pow2(-149), 32), 1)

    def test_model_classes_really_separate(self):
        self.assertEqual(len(design_classes(32)), 8)
        self.assertEqual(len(design_classes(16)), 2)
        self.assertTrue(any(r["kind"] == "rounding-discriminator" for r in vectors()))

    def test_vectors_bounds_and_categories(self):
        self.assertEqual(len(vectors()), N)
        self.assertTrue({"exact-anchor", "signed-zero", "subnormal-underflow", "finite-overflow", "masked-g-sentinel"} <= {r["kind"] for r in vectors()})
        self.assertTrue(all(finite(s, 32) for r in vectors() for s in r["source"]))

    def test_inputs_only_and_deterministic(self):
        a = inputs()
        self.assertEqual(a, inputs())
        self.assertEqual(set(a), {"cases.json", "q1-patterns.bin", "q2-source.bin", "q2-seed.bin"})
        self.assertEqual(len(a["q2-seed.bin"]), 3328)
        self.assertLess(len(a["cases.json"]), 131072)
        self.assertEqual(len(set(pattern(i) for i in range(3))), 3)

    def test_q1_native_records_guards_and_markers(self):
        d = bytearray([0xcd]) * Q1_BYTES
        r = dict(tag=7, precision=32)
        for i, b in enumerate(pattern(0)):
            struct.pack_into("<6I", d, TG + i * 24, 0x51710000 ^ 7, i, 0, 0, 0, quantize(Fraction(b, 255), 32))
        values = q1_values(r, d)
        q1_semantics(values, 0, "read-f32", 32)
        for offset in (0, TG, TG + 4, Q1_BYTES - 1):
            broken = bytearray(d); broken[offset] ^= 1
            with self.assertRaises(Refusal):
                q1_values(r, broken)

    def test_q1_wrong_channel_and_pattern_rejected(self):
        values = [(0, 0, 0, quantize(Fraction(v, 255), 16)) for v in pattern(0)]
        q1_semantics(values, 0, "read-f16", 16)
        with self.assertRaisesRegex(Refusal, "q1-channels"):
            q1_semantics([v[::-1] for v in values], 0, "read-f16", 16)
        with self.assertRaises(Refusal):
            q1_semantics(values, 1, "read-f16", 16)

    def test_positive_oracle_fixtures_memory_only(self):
        for s in specs()[:8]:
            ordinary_q2(*_fixture(s), s)

    def test_genuine_negative_design_rejected_by_same_oracle(self):
        expected = {"wrong-kernel": "return-source", "wrong-output": "exact-anchor-output",
                    "wrong-state": "exact-anchor-output", "wrong-mask": "preservation"}
        for s in specs()[8:]:
            with self.assertRaisesRegex(Refusal, expected[s["negative"]]):
                ordinary_q2(*_fixture(s), s, expected_mask=8 if s["negative"] == "wrong-mask" else 12, expected_mode="mul")

    def test_masked_green_payload_signedzero_and_padding(self):
        s = specs()[0]
        target, trace = _fixture(s)
        for offset in (0, GUARD + W * 4, TARGET_BYTES - 1, word_offset(0, 1),
                       word_offset(next(r["id"] for r in vectors() if r["dest"][1] == 0x7c01), 1)):
            damaged = bytearray(target); damaged[offset] ^= 1
            with self.assertRaisesRegex(Refusal, "preservation"):
                ordinary_q2(damaged, trace, s)

    def test_trace_corruptions_rejected(self):
        s = specs()[1]
        target, trace = _fixture(s)
        for offset in (0, TG, TG + 4, TG + 8, TG + 12, TRACE_BYTES - 1):
            damaged = bytearray(trace); damaged[offset] ^= 1
            with self.assertRaises(Refusal):
                ordinary_q2(target, damaged, s)

    def test_unknown_model_not_forced(self):
        s = specs()[1]
        target, trace = _fixture(s)
        summary, _ = analyze_models(target, trace, s)
        self.assertEqual(summary["matching_classes"], [["f32-mul-rtz"]])
        put16(target, word_offset(0, 0), 0x1111)
        summary, _ = analyze_models(target, trace, s)
        self.assertEqual(summary["matching_classes"], [])
        self.assertFalse(summary["resolved_among_enumerated_classes"])

    def test_half_physical_precision_not_overclaimed(self):
        s = specs()[5]
        summary, _ = analyze_models(*_fixture(s), s)
        self.assertEqual(len(summary["matching_classes"]), 1)
        self.assertGreater(len(summary["matching_classes"][0]), 1)
        self.assertFalse(summary["physical_intermediate_width_identified"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("generate", "validate", "self-test"))
    parser.add_argument("out", nargs="?", type=Path)
    args = parser.parse_args()
    if args.command == "self-test":
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests))
        return 0 if result.wasSuccessful() else 3
    require(args.out is not None, "output-directory-required")
    result = generate(args.out) if args.command == "generate" else validate(args.out)
    print(encode_json(result).decode())
    return 0 if args.command == "generate" or result["status"] == "BOUNDED_NATIVE_OBSERVATIONS_VALIDATED" else 3


if __name__ == "__main__":
    raise SystemExit(main())
