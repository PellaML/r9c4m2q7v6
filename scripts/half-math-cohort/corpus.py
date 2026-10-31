#!/usr/bin/env python3
"""Generic half-input cohort. Powr is raw observation, never a software golden."""
from collections import Counter
import itertools
import hashlib
import json
import os
from pathlib import Path
import struct
import sys

OPS = ("maximum_three_half", "power_half", "minimum_half3", "maximum_half3", "half3_widen_bits")
LANES = (1, 1, 3, 3, 3)
MAX_RECORDS = 32768
INPUT, SAMPLE = struct.Struct("<16I"), struct.Struct("<20I")
STAMP = 0x5AC3E271
ABI = (
    {"symbol": "air.fmax3.f16", "result": "half", "arguments": ["half", "half", "half"]},
    {"symbol": "air.powr.f16", "result": "half", "arguments": ["half", "half"]},
    {"symbol": "air.fmin.v3f16", "result": "<3 x half>", "arguments": ["<3 x half>", "<3 x half>"]},
    {"symbol": "air.fmax.v3f16", "result": "<3 x half>", "arguments": ["<3 x half>", "<3 x half>"]},
)
FLAGS = ("no_cpu_fallback", "device_created", "unified_memory", "pipeline_created",
         "buffers_created", "input_uploaded_exact", "output_poisoned_before_dispatch",
         "dispatch_encoded", "command_committed", "wait_until_completed_called",
         "command_completed", "readback_copied_after_completion", "output_guard_unchanged",
         "input_unchanged_after_completion")
ARTIFACTS = ("corpus.json", "inputs.bin", "uploaded-inputs.bin", "results.bin", "native.json",
             "check.json", "wrong-expected.json", "toolchain.txt", "half-math-cohort.air", "half-math-cohort.metallib")


def h(value):
    return struct.unpack("<H", struct.pack("<e", value))[0]


def classify(value, width=16):
    sign_bit, exponent = (0x8000, 0x7C00) if width == 16 else (0x80000000, 0x7F800000)
    magnitude = value & (sign_bit - 1)
    sign = "negative" if value & sign_bit else "positive"
    if magnitude > exponent: return "nan"
    if magnitude == exponent: return sign + "_infinity"
    if magnitude == 0: return sign + "_zero"
    if magnitude < (0x400 if width == 16 else 0x800000): return sign + "_subnormal"
    return "finite_normal"


def half_to_float_bits(value):
    """Independent lossless finite widening; no production converter or math library."""
    sign = (value & 0x8000) << 16
    exponent, fraction = (value >> 10) & 31, value & 1023
    if exponent == 31: raise ValueError("nonfinite widening has no exact anchor here")
    if exponent:
        return sign | ((exponent + 112) << 23) | (fraction << 13)
    if not fraction: return sign
    exponent = -14
    while fraction < 1024:
        fraction <<= 1
        exponent -= 1
    return sign | ((exponent + 127) << 23) | ((fraction & 1023) << 13)


def half_order(value):
    magnitude = value & 0x7FFF
    return -magnitude if value & 0x8000 else magnitude


def finite_patterns():
    values = dict.fromkeys(sign | (exponent << 10) | fraction
                         for sign in (0, 0x8000) for exponent in range(31)
                         for fraction in (0, 1, 0x155, 0x200, 0x3FE, 0x3FF))
    state = 0x8C5162AF
    for _ in range(128):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        state &= 0xFFFFFFFF
        value = state & 0xFFFF
        if value & 0x7C00 != 0x7C00: values.setdefault(value, None)
    return list(values)


def corpus():
    records = []

    def add(op, category, a, b=(0, 0, 0, 0), c=(0, 0, 0, 0), **extra):
        assert all(len(v) == 4 and all(0 <= x <= 0xFFFF for x in v) for v in (a, b, c))
        index = len(records)
        records.append({"operation": op, "category": category, "a": tuple(a), "b": tuple(b), "c": tuple(c),
                        "tag": (index * 0x9E3779B1 + 0x2468ACE1) & 0xFFFFFFFF, **extra})
        return index

    pool = finite_patterns()
    triples = {(h(1.0), h(2.0), h(3.0)): "exact_positive_control"}
    for index, value in enumerate(pool):
        peers = (value, pool[(73 * index + 17) % len(pool)], pool[(151 * index + 29) % len(pool)])
        for triple in itertools.permutations(peers): triples.setdefault(triple, "finite_pattern_permutations")
        if index % 5 == 0:
            for triple in itertools.permutations((value, value, peers[1])):
                triples.setdefault(triple, "two_way_tie_permutations")
            triples.setdefault((value,) * 3, "three_way_tie")
    for triple in itertools.product((0, 0x8000), repeat=3):
        triples.setdefault(triple, "signed_zero_ties_diagnostic")
    for triple in ((0, 0x8000, h(-1.0)), (0, 0x8000, h(1.0)), (1, 2, 3),
                   (0x8001, 0x8002, 0x8003), (0x3FF, 0x400, 0x401),
                   (0x7C00, 0xFC00, h(1.0)), (0x7E01, h(1.0), h(2.0)),
                   (0x7E01, 0xFE11, h(-1.0))):
        for ordered in itertools.permutations(triple): triples.setdefault(ordered, "domain_and_subnormal_diagnostic")
    for (a, b, c), category in triples.items(): add(0, category, (a, 0, 0, 0), (b, 0, 0, 0), (c, 0, 0, 0))

    pairs = {}
    bases = list(dict.fromkeys([0, 0x8000, 1, 2, 0x3FE, 0x3FF, 0x400, 0x401, 0x7BFE, 0x7BFF,
                               *[h(v) for v in (0.0625, 0.125, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5,
                                               2.0, 3.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 1024.0)],
                               *range(0x3BFE, 0x3C03), 0x37FF, 0x3801, 0x3FFF, 0x4001]))
    exponents = [0, 0x8000, 1, 0x8001]
    for value in (0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 8.0, 16.0, 32.0, 64.0,
                  128.0, 512.0, 1024.0, 2048.0, 8192.0, 32752.0, 65504.0):
        exponents.extend((h(value), h(-value)))
    for a, b in itertools.product(bases, exponents):
        category = "power_finite_grid"
        if a & 0x7FFF == 0: category = "power_signed_zero_base_diagnostic"
        elif half_order(b) > half_order(h(1024.0)) or half_order(b) < half_order(h(-1024.0)):
            category = "power_large_exponent_diagnostic"
        elif (a & 0x7FFF) <= 0x401: category = "power_small_base_subnormal_boundary"
        elif (a & 0x7FFF) >= 0x7BFE: category = "power_overflow_underflow_boundary"
        pairs.setdefault((a, b), category)
    for a, b in itertools.product((h(-1.0), h(-0.5), h(-16.0), h(-65504.0)),
                                 (h(-2.0), h(-1.0), h(-0.5), 0, h(0.5), h(1.0), h(2.0))):
        pairs.setdefault((a, b), "power_negative_base_domain_diagnostic")
    for special, value in itertools.product((0x7C00, 0xFC00, 0x7E01, 0xFE11), (0, 0x8000, h(0.5), h(1.0), h(-1.0))):
        pairs.setdefault((special, value), "power_nonfinite_diagnostic")
        pairs.setdefault((value, special), "power_nonfinite_diagnostic")
    for (a, b), category in pairs.items(): add(1, category, (a, 0, 0, 0), (b, 0, 0, 0))

    def vector_pair(op, category, a, b=(0, 0, 0)):
        # Fourth physical words are deliberately different, including NaNs. Only xyz are operands.
        first = add(op, category, (*a, 0x7E11), (*b, 0x8001), (0, 0, 0, 0x7BFF))
        add(op, category, (*a, 0xFC00), (*b, 0x3555), (0, 0, 0, 0), padding_peer=first)

    for op in (2, 3):
        for index, value in enumerate(pool):
            a = (value, pool[(17 * index + 7) % len(pool)], pool[(31 * index + 13) % len(pool)])
            b = (pool[(73 * index + 19) % len(pool)], a[1], pool[(97 * index + 23) % len(pool)])
            for rotation in range(3):
                vector_pair(op, "finite_half3_lane_rotations_and_ties", a[rotation:] + a[:rotation], b[rotation:] + b[:rotation])
        special_pairs = [(a, b) for a, b in itertools.product((0, 0x8000, 1, 0x8001, 0x3FF, 0x400, 0x7C00, 0xFC00, 0x7E01, 0xFE11),
                                                            (0, 0x8000, h(1.0), h(-1.0)))]
        for a, b in special_pairs:
            for rotation in range(3):
                left, right = (a, h(0.5), h(-2.0)), (b, h(0.5), h(3.0))
                vector_pair(op, "half3_zero_subnormal_nonfinite_lane_diagnostic",
                            left[rotation:] + left[:rotation], right[rotation:] + right[:rotation])
    for start in range(0, len(pool), 3):
        a = tuple((pool[start:start + 3] + [h(1.0)] * 3)[:3])
        for rotation in range(3):
            vector_pair(4, "exact_finite_half3_widening_bits", a[rotation:] + a[:rotation])
    assert 0 < len(records) <= MAX_RECORDS
    return records


def words(row):
    return (row["operation"], row["tag"], 0, 0, *row["a"], *row["b"], *row["c"])


def input_bytes(records):
    return b"".join(INPUT.pack(*words(row)) for row in records)


def stamp(index, row):
    result = STAMP ^ index ^ row["operation"] ^ row["tag"]
    for value in words(row)[4:]: result ^= value
    return result


def exact_controls(row):
    op = row["operation"]
    if op == 1: return {}  # No powr golden, including identities, domains or overflow.
    if op == 4: return {i: half_to_float_bits(row["a"][i]) for i in range(3)}
    expected = {}
    for lane in range(LANES[op]):
        values = [row["a"][lane], row["b"][lane]]
        if op == 0: values.append(row["c"][lane])
        if any(classify(v) not in ("finite_normal", "positive_zero", "negative_zero") for v in values): continue
        winner = (min if op == 2 else max)(values, key=half_order)
        if winner & 0x7FFF: expected[lane] = winner
    return expected


def manifest(records):
    data = input_bytes(records)
    controls = Counter()
    for row in records: controls[OPS[row["operation"]]] += len(exact_controls(row))
    return {"schema": 1, "cohort": "half-math", "records": len(records), "maximum_records": MAX_RECORDS,
            "input_stride": INPUT.size, "sample_stride": SAMPLE.size,
            "operation_records": {name: sum(r["operation"] == op for r in records) for op, name in enumerate(OPS)},
            "active_result_components": sum(LANES[r["operation"]] for r in records),
            "exact_controls_by_operation": dict(controls), "representative_finite_half_patterns": len(finite_patterns()),
            "padding_pairs": sum("padding_peer" in r for r in records), "inputs_sha256": hashlib.sha256(data).hexdigest(),
            "intended_math_air_prototypes": list(ABI), "conversion_control": "finite half3 -> float3, raw uint3 result bits",
            "input_encoding": "uint32 wire words containing exactly 16-bit IEEE half patterns, including unused fourth-word controls",
            "result_encoding": "operations 0..3: raw binary16 in low16 of uint32; operation4: binary32 widening bits; inactive words zero",
            "math_mode": "-std=metal4.0 -O2 -g0 -ffast-math",
            "air_status": "pending unmodified AIR declarations/calls; scalar f16 and v3f16 must not be inferred from source spelling",
            "powr_policy": "raw observations only; no CPU/software power, fitted table, identity golden or expected special-case result",
            "minmax_policy": "exact controls only for finite nonsubnormal operands with a nonzero winner; zeros/subnormals/nonfinite outcomes are diagnostics",
            "domain_policy": "diagnostic values never establish domain admission or justify relaxing guards",
            "cases": [{"record": i, "operation": OPS[r["operation"]], "category": r["category"],
                       "a": [f"{v:04x}" for v in r["a"]], "b": [f"{v:04x}" for v in r["b"]],
                       "c": [f"{v:04x}" for v in r["c"]], "tag": f"{r['tag']:08x}",
                       **({"padding_peer": r["padding_peer"]} if "padding_peer" in r else {})}
                      for i, r in enumerate(records)]}


def validate_native(native, records):
    expected = {"schema": 1, "cohort": "half-math", "api": "Metal compute", "architecture": "arm64",
                "availability": "available_and_executed", "phase": "completed", "exit_code": 0,
                "error": None, "command_status": 4, "threads": len(records),
                "input_bytes": len(records) * INPUT.size, "result_bytes": len(records) * SAMPLE.size,
                "operation_records": [sum(r["operation"] == op for r in records) for op in range(5)]}
    if any(native.get(k) != v for k, v in expected.items()): raise ValueError("native completion metadata mismatch")
    if native.get("os_major") not in (26, 27) or not native.get("device_name"): raise ValueError("native platform missing")
    if any(native.get(k) is not True for k in FLAGS): raise ValueError("execution flags must be true JSON booleans")


def examine(records, data, wrong=False):
    if len(data) != len(records) * SAMPLE.size: raise ValueError("result byte length mismatch")
    samples = list(SAMPLE.iter_unpack(data))
    identity = inactive = encoding = controls = mismatches = 0
    first = []
    classes = {name: Counter() for name in OPS}
    counts = {name: Counter() for name in OPS}
    padding = Counter()
    for index, (row, sample) in enumerate(zip(records, samples)):
        op, lanes = row["operation"], LANES[row["operation"]]
        if sample[:16] != (op, index, row["tag"], stamp(index, row), *words(row)[4:]): identity += 1
        if any(sample[16 + lanes:]): inactive += 1
        if op != 4 and any(v > 0xFFFF for v in sample[16:16 + lanes]): encoding += 1
        for value in sample[16:16 + lanes]: classes[OPS[op]][classify(value, 32 if op == 4 else 16)] += 1
        for lane, expected in exact_controls(row).items():
            if wrong and controls == 0: expected ^= 1  # Expected only; never mutate a native byte.
            controls += 1
            counts[OPS[op]]["controls"] += 1
            if sample[16 + lane] != expected:
                mismatches += 1
                counts[OPS[op]]["mismatches"] += 1
                if len(first) < 12:
                    first.append({"record": index, "lane": lane, "operation": OPS[op],
                                  "expected": f"{expected:08x}", "actual": f"{sample[16 + lane]:08x}"})
        if "padding_peer" in row:
            peer = samples[row["padding_peer"]]
            padding["equal" if sample[16:16 + lanes] == peer[16:16 + lanes] else "different"] += 1
    return {"schema": 1, "scope": "execution integrity, unambiguous min/max and finite bit-conversion controls only",
            "records": len(records), "identity_errors": identity, "inactive_lane_errors": inactive,
            "half_result_encoding_errors": encoding, "exact_controls": controls, "exact_control_mismatches": mismatches,
            "controls_by_operation": {k: dict(v) for k, v in counts.items()}, "first_control_mismatches": first,
            "wrong_expected_control": wrong, "output_classes": {k: dict(v) for k, v in classes.items()},
            "unused_fourth_word_pair_observations": dict(padding), "padding_diagnostic_equality_is_not_a_gate": True,
            "powr_goldens": 0, "software_or_full_domain_parity_established": False,
            "air_status": "pending unmodified AIR inspection",
            "integrity_and_exact_controls_pass": bool(controls) and not (identity or inactive or encoding or mismatches)}


def write_json(path, value):
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def generate(out):
    out.mkdir(parents=True, exist_ok=True)
    if out.is_symlink(): raise ValueError("output cannot be a symlink")
    records = corpus()
    with (out / "inputs.bin").open("xb") as stream: stream.write(input_bytes(records))
    write_json(out / "corpus.json", manifest(records))


def check(out, wrong):
    records = corpus()
    inputs = input_bytes(records)
    if any((out / name).read_bytes() != inputs for name in ("inputs.bin", "uploaded-inputs.bin")): raise ValueError("input identity mismatch")
    if json.loads((out / "corpus.json").read_text()) != manifest(records): raise ValueError("corpus changed")
    validate_native(json.loads((out / "native.json").read_text()), records)
    data = (out / "results.bin").read_bytes()
    result = examine(records, data, wrong)
    result["results_sha256"] = hashlib.sha256(data).hexdigest()
    result["inputs_sha256"] = hashlib.sha256(inputs).hexdigest()
    if wrong:
        baseline = examine(records, data)
        result["control_valid"] = baseline["integrity_and_exact_controls_pass"] and result["exact_control_mismatches"] == 1
    write_json(out / ("wrong-expected.json" if wrong else "check.json"), result)
    if wrong: return 1 if result["control_valid"] else 3
    return 0 if result["integrity_and_exact_controls_pass"] else 1


def audit(out):
    root, home = str(Path.cwd().resolve()), os.environ.get("HOME", "")
    repository = os.environ.get("GITHUB_REPOSITORY", "").split("/")
    public = (len(repository) == 2 and all(repository) and os.environ.get("GITHUB_ACTIONS") == "true"
              and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
              and root == f"/Users/runner/work/{repository[-1]}/{repository[-1]}"
              and os.environ.get("GITHUB_WORKSPACE") == root and home == "/Users/runner")
    for name in ARTIFACTS:
        data = (out / name).read_bytes()
        if not 0 < len(data) <= 8 * 1024 * 1024: raise ValueError("empty or oversized artifact")
        if not public:
            for path in (root, home):
                if len(path) > 1 and any(path.encode(e) in data for e in ("utf-8", "utf-16le", "utf-16be")):
                    raise ValueError("nonpublic path in artifact")


def selftest():
    assert INPUT.size == 64 and SAMPLE.size == 80 and len(ABI[0]["arguments"]) == 3
    for source, expected in ((0, 0), (0x8000, 0x80000000), (1, 0x33800000), (0x8001, 0xB3800000),
                             (0x3FF, 0x387FC000), (0x400, 0x38800000), (0x3C00, 0x3F800000), (0x7BFF, 0x477FE000)):
        assert half_to_float_bits(source) == expected
    for value in finite_patterns():
        decoded = struct.unpack("<e", struct.pack("<H", value))[0]
        assert half_to_float_bits(value) == struct.unpack("<I", struct.pack("<f", decoded))[0]
    records = corpus()
    assert records == corpus() and len(records) <= MAX_RECORDS and set(r["operation"] for r in records) == set(range(5))
    assert any(r["operation"] == 1 and r["b"][0] == h(0.5) for r in records)
    assert all(any(r["operation"] == 1 and r["b"][0] == h(value) for r in records) for value in (65504.0, -65504.0))
    assert all(not exact_controls(r) for r in records if r["operation"] == 1)
    synthetic = []
    for index, row in enumerate(records):
        values = [h(0.5)] * LANES[row["operation"]] + [0] * (4 - LANES[row["operation"]])
        for lane, expected in exact_controls(row).items(): values[lane] = expected
        synthetic.append(SAMPLE.pack(row["operation"], index, row["tag"], stamp(index, row), *words(row)[4:], *values))
        if "padding_peer" in row:
            peer = records[row["padding_peer"]]
            assert peer["operation"] == row["operation"]
            assert all(peer[key][:3] == row[key][:3] for key in ("a", "b", "c"))
            assert peer["a"][3] != row["a"][3]
    data = b"".join(synthetic)
    assert examine(records, data)["integrity_and_exact_controls_pass"]
    assert examine(records, data, True)["exact_control_mismatches"] == 1
    broken = bytearray(data); broken[4] ^= 1
    assert examine(records, broken)["identity_errors"] == 1
    broken = bytearray(data); broken[66] ^= 1
    assert examine(records, broken)["half_result_encoding_errors"] == 1
    broken = bytearray(data); broken[76] = 1
    assert examine(records, broken)["inactive_lane_errors"] == 1
    assert examine(records, bytes([0xA5]) * len(data))["identity_errors"] == len(records)
    try: examine(records, data[:-1])
    except ValueError: pass
    else: raise AssertionError("truncation accepted")
    fake = {k: True for k in FLAGS}
    fake.update(schema=1, cohort="half-math", api="Metal compute", architecture="arm64", availability="available_and_executed",
                phase="completed", exit_code=0, error=None, command_status=4, threads=len(records),
                input_bytes=len(records)*INPUT.size, result_bytes=len(records)*SAMPLE.size,
                operation_records=[sum(r["operation"] == op for r in records) for op in range(5)],
                os_major=26, device_name="synthetic unit test; not native evidence")
    validate_native(fake, records)
    for key, bad in (("command_status", 3), ("pipeline_created", 1), ("readback_copied_after_completion", False),
                     ("output_guard_unchanged", False), ("operation_records", [0]*5), ("cohort", "different")):
        try: validate_native({**fake, key: bad}, records)
        except ValueError: pass
        else: raise AssertionError("bad native metadata accepted")
    info = manifest(records)
    assert len(json.dumps(info, indent=2, sort_keys=True).encode()) < 8 * 1024 * 1024
    print(json.dumps({"status": "LOCAL_SYNTHETIC_CHECKS_ONLY_NOT_NATIVE", "records": len(records),
                      "operation_records": info["operation_records"], "active_result_components": info["active_result_components"],
                      "representative_finite_half_patterns": len(finite_patterns()), "padding_pairs": info["padding_pairs"],
                      "exact_controls_by_operation": info["exact_controls_by_operation"], "powr_goldens": 0,
                      "wrong_expectation_identity_encoding_inactive_poison_truncation_and_metadata_controls": "pass"}, sort_keys=True))


def main():
    if sys.argv[1:] == ["selftest"]: selftest(); return 0
    if len(sys.argv) != 3: raise ValueError("usage: selftest | generate/check/wrong-expected/audit DIRECTORY")
    command, out = sys.argv[1], Path(sys.argv[2])
    if command == "generate": generate(out); return 0
    if command == "check": return check(out, False)
    if command == "wrong-expected": return check(out, True)
    if command == "audit": audit(out); return 0
    raise ValueError("unknown command")


if __name__ == "__main__":
    try: status = main()
    except (ValueError, OSError, AssertionError, KeyError, TypeError, struct.error):
        print("half cohort refused: integrity, exact control or privacy failure", file=sys.stderr)
        status = 3
    sys.exit(status)
