#!/usr/bin/env python3
"""Finite IEEE-754 fractional-part corpus and independent, explicit bit references."""
import functools
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import sys

COUNT = 63488
STAMP = 0x91F6A37C
INPUT = struct.Struct("<2I4H")
SAMPLE = struct.Struct("<4I6H3I")
ARTIFACTS = ("inputs.bin", "samples.bin", "corpus.json", "native.json", "check.json",
             "wrong-expected.json", "toolchain.txt", "fract.air", "fract.metallib")


def encode(value, width):
    return int.from_bytes(struct.pack("<f" if width == 32 else "<e", value), "little")


def decode(bits, width):
    return struct.unpack("<f" if width == 32 else "<e", bits.to_bytes(width // 8, "little"))[0]


def subnormal(bits, width):
    fraction, exponent = (0x7FFFFF, 0x7F800000) if width == 32 else (0x3FF, 0x7C00)
    return (bits & exponent) == 0 and (bits & fraction) != 0


def curated():
    values = [0, 0x80000000, 1, 2, 0x7FFFFF, 0x800000, 0x800001,
              0x80000001, 0x807FFFFF, 0x80800000, 0x80800001,
              0xB2FFFFFF, 0xB3000000, 0xB3000001, 0xB37FFFFF, 0xB3800000, 0xB3800001,
              0x7F7FFFFF, 0xFF7FFFFF]
    for number in (0.125, 0.25, 0.5, 1, 2, 3, 7, 16, 256, 65536, 2**22, 2**23, 2**24):
        base = encode(number, 32)
        for sign in (0, 0x80000000):
            values.extend((base + delta) | sign for delta in (-2, -1, 0, 1, 2))
    return list(dict.fromkeys(values))


def corpus():
    floats = curated()
    for sign in (0, 0x80000000):
        for exponent in range(255):
            for fraction in (0, 1, 0x3FFFFF, 0x7FFFFE, 0x7FFFFF):
                floats.append(sign | (exponent << 23) | fraction)
    state = 0x6D2B79F5
    while len(floats) < COUNT * 2:
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        state &= 0xFFFFFFFF
        bits = state ^ (0x800000 if state & 0x7F800000 == 0x7F800000 else 0)
        floats.append(bits)
    halves = [b for b in range(65536) if b & 0x7C00 != 0x7C00]
    assert len(halves) == COUNT
    return [(floats[2*i], floats[2*i+1], halves[i], halves[(17*i+1) % COUNT],
             halves[(73*i+129) % COUNT], 0) for i in range(COUNT)]


def models(width):
    clamps = ("native_width",) if width == 32 else ("native_width", "float_clamp_then_half")
    return [(clamp, flush_in, flush_out) for clamp in clamps for flush_in in (False, True)
            for flush_out in (False, True)]


def label(model):
    clamp, flush_in, flush_out = model
    return f"{clamp}:input_{'ftz' if flush_in else 'preserve'}:output_{'ftz' if flush_out else 'preserve'}"


@functools.lru_cache(maxsize=600000)
def reference(bits, width, model):
    clamp, flush_in, flush_out = model
    if flush_in and subnormal(bits, width):
        bits &= 1 << (width - 1)
    x = decode(bits, width)
    assert math.isfinite(x)
    # Preserve floor(-0) as -0 before subtraction; Python floor returns integer0.
    floor_x = x if x == 0.0 else math.floor(x)
    fraction = x - floor_x
    cap = decode(0x3F7FFFFF, 32) if width == 32 or clamp == "float_clamp_then_half" else decode(0x3BFF, 16)
    result = encode(min(fraction, cap), width)
    if flush_out and subnormal(result, width):
        result &= 1 << (width - 1)
    return result


def stamp(index, row):
    f0, f1, h0, h1, h2, _ = row
    return f0 ^ f1 ^ (h0 << 16) ^ h1 ^ h2 ^ index ^ STAMP


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def generate(out):
    out.mkdir(parents=True, exist_ok=False)
    rows = corpus()
    data = b"".join(INPUT.pack(*row) for row in rows)
    (out / "inputs.bin").write_bytes(data)
    write_json(out / "corpus.json", {"schema": 1, "records": COUNT, "input_stride": INPUT.size,
        "sample_stride": SAMPLE.size, "f32_cells": COUNT*2, "f16_cells": COUNT*3,
        "f16_coverage": "all63488 finite patterns once in each of three permuted lanes",
        "f32_coverage": "curated edges, all finite exponent boundaries, deterministic xorshift32",
        "curated_f32_bits": [f"{b:08x}" for b in curated()], "inputs_sha256": hashlib.sha256(data).hexdigest(),
        "excluded": ["NaN", "+infinity", "-infinity"], "compiler_math": "fast",
        "candidate_models": {str(w): [label(m) for m in models(w)] for w in (32, 16)}})


def examine(rows, samples, wrong=False):
    mismatches = {w: {label(m): 0 for m in models(w)} for w in (32, 16)}
    first = {w: {label(m): [] for m in models(w)} for w in (32, 16)}
    identity_errors = 0
    for index, (row, sample) in enumerate(zip(rows, samples)):
        f_in, f_out, h_in, h_out = sample[:2], sample[2:4], sample[4:7], sample[7:10]
        if tuple(f_in) != row[:2] or tuple(h_in) != row[2:5] or sample[10:] != (index, stamp(index, row), 0):
            identity_errors += 1
        # References use the original corpus, never an untrusted device echo.
        for width, inputs, outputs in ((32, row[:2], f_out), (16, row[2:5], h_out)):
            for lane, (bits, actual) in enumerate(zip(inputs, outputs)):
                for model in models(width):
                    expected = reference(bits, width, model)
                    if wrong and index == 0 and lane == 0 and width == 32:
                        expected ^= 1  # Change one expectation, never a native byte.
                    key = label(model)
                    if expected != actual:
                        mismatches[width][key] += 1
                        if len(first[width][key]) < 8:
                            first[width][key].append({"record": index, "lane": lane, "input": f"{bits:0{width//4}x}",
                                "expected": f"{expected:0{width//4}x}", "actual": f"{actual:0{width//4}x}"})
    matching = {w: [name for name, count in mismatches[w].items() if count == 0] for w in (32, 16)}
    return {"schema": 1, "records": len(samples), "identity_errors": identity_errors,
            "wrong_expected_control": wrong, "mismatches": mismatches, "first_mismatches": first,
            "matching_models": matching, "numeric_acceptance": bool(matching[32] and matching[16]) and not identity_errors}


def check(out, wrong):
    rows = corpus()
    expected_inputs = b"".join(INPUT.pack(*row) for row in rows)
    if (out / "inputs.bin").read_bytes() != expected_inputs:
        raise ValueError("input identity mismatch")
    data = (out / "samples.bin").read_bytes()
    if len(data) != COUNT * SAMPLE.size:
        raise ValueError("sample size mismatch")
    native = json.loads((out / "native.json").read_text())
    if not (native["availability"] == "available_and_executed" and native["phase"] == "completed"
            and native["command_status"] == 4 and native["error"] is None
            and native["pipeline_created"] and native["wait_until_completed_called"]
            and native["readback_copied_after_successful_wait"] and native["threads"] == COUNT):
        raise ValueError("native completion join missing")
    result = examine(rows, list(SAMPLE.iter_unpack(data)), wrong)
    result["samples_sha256"] = hashlib.sha256(data).hexdigest()
    write_json(out / ("wrong-expected.json" if wrong else "check.json"), result)
    if wrong:
        assert not result["numeric_acceptance"]
        assert result["identity_errors"] == 0 and min(result["mismatches"][32].values()) == 1
        assert min(result["mismatches"][16].values()) == 0
    return 0 if result["numeric_acceptance"] else 1


def audit(out):
    root, home = str(Path.cwd().resolve()), os.environ.get("HOME", "")
    public = False
    repository = os.environ.get("GITHUB_REPOSITORY", "").split("/")
    if len(repository) == 2 and all(repository):
        expected = f"/Users/runner/work/{repository[1]}/{repository[1]}"
        public = (os.environ.get("GITHUB_ACTIONS") == "true"
                  and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
                  and root == expected and home == "/Users/runner"
                  and os.environ.get("GITHUB_WORKSPACE") == root)
    for name in ARTIFACTS:
        data = (out / name).read_bytes()
        if not 0 < len(data) <= 4*1024*1024:
            raise ValueError("artifact empty or oversized")
        if not public:
            for path in (root, home):
                if len(path) > 1 and any(path.encode(e) in data for e in ("utf-8", "utf-16le", "utf-16be")):
                    raise ValueError("nonpublic local path in artifact")


def selftest():
    assert INPUT.size == 16 and SAMPLE.size == 40
    rows = corpus()
    assert len(rows) == COUNT and rows[0][:2] == (0, 0x80000000)
    for lane in range(2, 5):
        assert len({r[lane] for r in rows}) == COUNT
    preserve = ("native_width", False, False)
    ftz = ("native_width", True, True)
    assert reference(0xB3000000, 32, preserve) == 0x3F7FFFFF
    assert reference(0x80000001, 32, preserve) == 0x3F7FFFFF
    assert reference(0x80000001, 32, ftz) == 0
    assert reference(0x8001, 16, preserve) == 0x3BFF
    assert reference(0x8001, 16, ("float_clamp_then_half", False, False)) == 0x3C00
    row = rows[0]
    sample = (*row[:2], *(reference(x, 32, ftz) for x in row[:2]), *row[2:5],
              *(reference(x, 16, preserve) for x in row[2:5]), 0, stamp(0, row), 0)
    assert examine([row], [sample])["numeric_acceptance"]
    wrong = examine([row], [sample], True)
    assert not wrong["numeric_acceptance"] and min(wrong["mismatches"][32].values()) == 1
    broken = list(sample); broken[-1] = 1
    assert examine([row], [tuple(broken)])["identity_errors"] == 1
    print("finite corpus, lane coverage, independent-width references and rejection controls passed")


def main():
    if sys.argv[1:] == ["selftest"]:
        selftest(); return 0
    if len(sys.argv) != 3:
        raise ValueError("usage: selftest | generate/check/wrong-expected/audit DIRECTORY")
    command, out = sys.argv[1], Path(sys.argv[2])
    if command == "generate": generate(out); return 0
    if command == "check": return check(out, False)
    if command == "wrong-expected": return check(out, True)
    if command == "audit": audit(out); return 0
    raise ValueError("unknown operation")


if __name__ == "__main__":
    try:
        status = main()
    except (ValueError, OSError, AssertionError, KeyError, struct.error):
        print("fraction probe refused: integrity, numeric or privacy check failed", file=sys.stderr)
        status = 3
    sys.exit(status)
