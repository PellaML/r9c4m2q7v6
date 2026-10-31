#!/usr/bin/env python3
"""Generic bit-input math cohort; raw SFU observations, not a CPU math oracle."""
from collections import Counter
import hashlib
import itertools
import json
import os
from pathlib import Path
import struct
import sys

OPS = ("power_scalar", "power_vector", "exponential", "logarithm", "maximum_three")
AIR_TARGETS = ("air.fast_powr.f32", "air.fast_powr.v4f32", "air.fast_exp.f32",
               "air.fast_log.f32", "air.fast_fmax3.f32")
INPUT = struct.Struct("<16I")
SAMPLE = struct.Struct("<20I")
STAMP = 0x5AC3E271
ONE = 0x3F800000
FLAGS = ("no_cpu_fallback", "device_created", "unified_memory", "pipeline_created",
         "buffers_created", "input_uploaded_exact", "output_poisoned_before_dispatch",
         "dispatch_encoded", "command_committed", "wait_until_completed_called",
         "command_completed", "readback_copied_after_completion", "output_guard_unchanged",
         "input_unchanged_after_completion")
ARTIFACTS = ("corpus.json", "inputs.bin", "uploaded-inputs.bin", "results.bin", "native.json",
             "check.json", "wrong-expected.json", "toolchain.txt", "math-cohort.air", "math-cohort.metallib")


def bits(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def number(value):
    return struct.unpack("<f", struct.pack("<I", value))[0]


def classification(value):
    magnitude = value & 0x7FFFFFFF
    sign = "negative" if value >> 31 else "positive"
    if magnitude > 0x7F800000:
        return "nan"
    if magnitude == 0x7F800000:
        return sign + "_infinity"
    if not magnitude:
        return sign + "_zero"
    if magnitude < 0x800000:
        return sign + "_subnormal"
    return "finite_normal"


def corpus():
    records = []

    def add(operation, category, a, b=(0, 0, 0, 0), c=(0, 0, 0, 0), **extra):
        assert all(len(v) == 4 for v in (a, b, c))
        index = len(records)
        records.append({"operation": operation, "category": category,
                        "tag": (index * 0x9E3779B1 + 0x2468ACE1) & 0xFFFFFFFF,
                        "a": tuple(a), "b": tuple(b), "c": tuple(c), **extra})
        return index

    pairs = {}

    def pair(a, b, category):
        pairs.setdefault((a, b), category)

    for exponent in (1.0, 2.0, -1.0, 0.0, -0.0):
        pair(ONE, bits(exponent), "unit_identity")
    bases = (2**-16, 2**-8, 0.125, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0, 16.0, 256.0)
    exponents = (-16.0, -8.0, -4.0, -3.0, -2.0, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 8.0, 16.0)
    for base, exponent in itertools.product(bases, exponents):
        pair(bits(base), bits(exponent), "ordinary_finite_grid")
    for delta, exponent in itertools.product(range(-4, 5), (-65536.0, -255.5, -8.0, -0.5, 0.5, 8.0, 255.5, 65536.0)):
        pair(ONE + delta, bits(exponent), "near_one_large_and_fractional_exponent")
    for base, exponent in itertools.product((1, 2, 0x7FFFFF, 0x800000, 0x800001, 0x7F7FFFFF), (-2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 4.0)):
        pair(base, bits(exponent), "positive_domain_extremes")
    for base, exponent in itertools.product((0, 0x80000000), (-2.0, -0.5, -0.0, 0.0, 0.5, 1.0, 2.0)):
        pair(base, bits(exponent), "signed_zero_base_diagnostic")
    for base in bases:
        pair(bits(base), 0x80000000, "negative_zero_exponent_diagnostic")
    for base, exponent in itertools.product((0xBF800000, 0x80800000, 0xFF7FFFFF), (-1.0, 0.0, 0.5, 2.0)):
        pair(base, bits(exponent), "outside_nonnegative_base_domain")
    for special, ordinary in itertools.product((0x7F800000, 0xFF800000, 0x7FC12345, 0xFFC54321), (0.0, 1.0, 2.0)):
        pair(special, bits(ordinary), "nonfinite_diagnostic")
        pair(bits(ordinary), special, "nonfinite_diagnostic")
    scalar_indices = []
    pair_items = list(pairs.items())
    for (a, b), category in pair_items:
        scalar_indices.append(add(0, category, (a, 0, 0, 0), (b, 0, 0, 0)))
    # Every scalar pair appears in every vector lane. Tail padding repeats the first pair.
    for start in range(0, len(pair_items), 4):
        indices = list(range(start, min(start + 4, len(pair_items))))
        indices += [0] * (4 - len(indices))
        for rotation in range(4):
            rotated = indices[rotation:] + indices[:rotation]
            add(1, "paired_scalar_vector_lanes", tuple(pair_items[i][0][0] for i in rotated),
                tuple(pair_items[i][0][1] for i in rotated),
                scalar_links=[scalar_indices[i] for i in rotated],
                lane_categories=[pair_items[i][1] for i in rotated])

    exp_inputs = {0: "signed_zero", 0x80000000: "signed_zero"}
    for numerator in range(-256, 257):
        exp_inputs.setdefault(bits(numerator / 16.0), "ordinary_finite_grid")
    for value in (-16.0, -2.0, -1.0, -0.6931471805599453, -0.5, 0.5, 0.6931471805599453, 1.0, 2.0, 16.0):
        for delta in range(-3, 4):
            exp_inputs.setdefault(bits(value) + delta, "ordinary_ulp_neighbors")
    # Rounded neighborhoods of mathematical underflow/overflow limits, not fitted cutoffs.
    for value in (-104.0, -103.97208, -103.27893, -87.33655, 88.0, 88.72284, 89.0):
        for delta in range(-3, 4):
            exp_inputs.setdefault(bits(value) + delta, "range_limit_neighborhood")
    for value in (1, 2, 0x7FFFFF, 0x800000, 0x800001, 0x7F7FFFFF,
                  0x80000001, 0x807FFFFF, 0x80800000, 0xFF7FFFFF,
                  0x7F800000, 0xFF800000, 0x7FC12345, 0xFFC54321):
        exp_inputs.setdefault(value, "domain_extreme_or_nonfinite_diagnostic")
    for value, category in exp_inputs.items():
        add(2, category, (value, 0, 0, 0))

    log_inputs = {ONE: "unit_identity", 0: "signed_zero_domain_limit", 0x80000000: "signed_zero_domain_limit"}
    for exponent, fraction in itertools.product(range(1, 255), (0, 1, 0x400000, 0x7FFFFF)):
        log_inputs.setdefault((exponent << 23) | fraction, "positive_normal_exponent_grid")
    for delta in range(-16, 17):
        log_inputs.setdefault(ONE + delta, "near_one_ulp_neighbors")
    for numerator in range(1, 65):
        log_inputs.setdefault(bits(numerator / 8.0), "ordinary_positive_grid")
    for value in (1, 2, 3, 0x100, 0x400000, 0x7FFFFE, 0x7FFFFF):
        log_inputs.setdefault(value, "positive_subnormal_domain_limit")
    for value in (0x80000001, 0x80800000, 0xBF000000, 0xBF800000, 0xC1800000, 0xFF7FFFFF,
                  0x7F800000, 0xFF800000, 0x7FC12345, 0xFFC54321):
        log_inputs.setdefault(value, "outside_positive_finite_domain")
    for value, category in log_inputs.items():
        add(3, category, (value, 0, 0, 0))

    triples = {(bits(1.0), bits(2.0), bits(3.0)): "exact_positive_control"}

    def permutations(values, category):
        for triple in itertools.permutations(values):
            triples.setdefault(triple, category)

    for values in ((0.25, 0.5, 0.75), (-4.0, -2.0, -0.5), (65504.0, -65504.0, 1.0),
                   (2**-126, 2**-125, 2**-124), (0.1, 0.2, 0.3)):
        permutations(tuple(bits(v) for v in values), "finite_argument_permutations")
    permutations((0x7F7FFFFF, bits(65504.0), ONE), "finite_maximum_boundary")
    permutations((ONE - 1, ONE, ONE + 1), "adjacent_float_arguments")
    for high, low in ((-3.0, -4.0), (-0.5, -1.0), (0.5, 0.25), (1.0, 0.5), (8.0, 4.0), (65504.0, 1.0)):
        permutations((bits(high), bits(high), bits(low)), "equal_maximum_ties")
        triples.setdefault((bits(high),) * 3, "three_way_ties")
    for triple in itertools.product((0, 0x80000000), repeat=3):
        triples.setdefault(triple, "signed_zero_ties_diagnostic")
    for triple in ((0, 0x80000000, bits(-1.0)), (0, 0x80000000, ONE),
                   (0x80000000, bits(-1.0), bits(-2.0)), (1, 2, 3),
                   (0x80000001, 0x80000002, 0x80000003), (0x7FFFFF, 0x800000, 0x800001),
                   (0x7FC12345, ONE, bits(2.0)), (0x7F800000, 0xFF800000, ONE),
                   (0x7FC12345, 0xFFC54321, bits(-1.0))):
        permutations(triple, "zero_subnormal_or_nonfinite_diagnostic")
    for (a, b, c), category in triples.items():
        add(4, category, (a, 0, 0, 0), (b, 0, 0, 0), (c, 0, 0, 0))
    assert 0 < len(records) <= 8192
    return records


def input_words(row):
    return (row["operation"], row["tag"], 0, 0, *row["a"], *row["b"], *row["c"])


def input_bytes(records):
    return b"".join(INPUT.pack(*input_words(row)) for row in records)


def stamp(index, row):
    words = input_words(row)
    result = STAMP ^ index ^ words[0] ^ words[1]
    for value in words[4:]:
        result ^= value
    return result


def exact_control(row):
    if row["operation"] != 4:
        return None
    values = (row["a"][0], row["b"][0], row["c"][0])
    if any(classification(v) not in ("finite_normal", "positive_zero", "negative_zero") for v in values):
        return None
    winner = max(values, key=number)
    # Signed-zero ties and denormal/NaN/Inf behavior are intentionally unqualified.
    return winner if number(winner) != 0.0 else None


def manifest(records):
    data = input_bytes(records)
    return {"schema": 1, "records": len(records), "input_stride": INPUT.size, "sample_stride": SAMPLE.size,
            "operation_records": {name: sum(r["operation"] == op for r in records) for op, name in enumerate(OPS)},
            "result_components": sum(4 if r["operation"] == 1 else 1 for r in records),
            "exact_finite_maximum_controls": sum(exact_control(r) is not None for r in records),
            "inputs_sha256": hashlib.sha256(data).hexdigest(), "intended_air_signatures": list(AIR_TARGETS),
            "air_signature_verification": "pending inspection of the unmodified compiled AIR; source spelling alone is insufficient",
            "math_mode": "-std=metal4.0 -O2 -g0 -ffast-math",
            "transcendental_policy": "raw observations only; no CPU library, fitted table or software-core outputs as expected values",
            "domain_policy": "zero, negative-domain, subnormal and nonfinite diagnostics do not establish supported semantics",
            "cases": [{"record": i, "operation": OPS[r["operation"]], "category": r["category"],
                       "a": [f"{v:08x}" for v in r["a"]], "b": [f"{v:08x}" for v in r["b"]],
                       "c": [f"{v:08x}" for v in r["c"]], "tag": f"{r['tag']:08x}",
                       **({"scalar_links": r["scalar_links"], "lane_categories": r["lane_categories"]} if r["operation"] == 1 else {})}
                      for i, r in enumerate(records)]}


def validate_native(native, records):
    expected = {"schema": 1, "api": "Metal compute", "architecture": "arm64",
                "availability": "available_and_executed", "phase": "completed", "exit_code": 0,
                "error": None, "command_status": 4, "threads": len(records),
                "input_bytes": len(records) * INPUT.size, "result_bytes": len(records) * SAMPLE.size,
                "operation_records": [sum(r["operation"] == op for r in records) for op in range(5)]}
    if any(native.get(key) != value for key, value in expected.items()):
        raise ValueError("native completion metadata mismatch")
    if native.get("os_major") not in (26, 27) or not native.get("device_name"):
        raise ValueError("native platform identity missing")
    if any(native.get(key) is not True for key in FLAGS):
        raise ValueError("native execution flags must be true JSON booleans")


def examine(records, data, wrong=False):
    if len(data) != len(records) * SAMPLE.size:
        raise ValueError("result byte length mismatch")
    samples = list(SAMPLE.iter_unpack(data))
    identity_errors = inactive_lane_errors = control_errors = controls = 0
    first_errors = []
    classes = {op: Counter() for op in OPS}
    for index, (row, sample) in enumerate(zip(records, samples)):
        if sample[:16] != (row["operation"], index, row["tag"], stamp(index, row), *input_words(row)[4:]):
            identity_errors += 1
        lanes = 4 if row["operation"] == 1 else 1
        if any(sample[16 + lanes:]):
            inactive_lane_errors += 1
        for value in sample[16:16 + lanes]:
            classes[OPS[row["operation"]]][classification(value)] += 1
        expected = exact_control(row)
        if expected is not None:
            if wrong and controls == 0:
                expected ^= 1  # One wrong expectation only; raw native bytes never change.
            controls += 1
            if sample[16] != expected:
                control_errors += 1
                if len(first_errors) < 12:
                    first_errors.append({"record": index, "expected": f"{expected:08x}", "actual": f"{sample[16]:08x}"})
    paired = Counter()
    paired_errors = []
    for index, row in enumerate(records):
        if row["operation"] != 1:
            continue
        for lane, scalar in enumerate(row["scalar_links"]):
            equal = samples[index][16 + lane] == samples[scalar][16]
            paired["equal" if equal else "different"] += 1
            if not equal and len(paired_errors) < 16:
                paired_errors.append({"vector_record": index, "lane": lane, "scalar_record": scalar,
                                      "category": row["lane_categories"][lane],
                                      "vector_bits": f"{samples[index][16 + lane]:08x}",
                                      "scalar_bits": f"{samples[scalar][16]:08x}"})
    return {"schema": 1, "scope": "execution integrity and exact finite maximum controls only",
            "records": len(records), "identity_errors": identity_errors, "inactive_lane_errors": inactive_lane_errors,
            "exact_controls": controls, "exact_control_mismatches": control_errors, "first_control_mismatches": first_errors,
            "wrong_expected_control": wrong, "output_classes": {k: dict(v) for k, v in classes.items()},
            "scalar_vector_power_observations": dict(paired), "first_scalar_vector_differences": paired_errors,
            "scalar_vector_equality_is_not_a_gate": True, "software_or_full_domain_parity_established": False,
            "air_signature_verification": "pending unmodified AIR inspection",
            "integrity_and_exact_controls_pass": bool(controls) and not (identity_errors or inactive_lane_errors or control_errors)}


def write_json(path, value):
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def generate(out):
    out.mkdir(parents=True, exist_ok=True)
    if out.is_symlink():
        raise ValueError("output directory cannot be a symlink")
    records = corpus()
    with (out / "inputs.bin").open("xb") as stream:
        stream.write(input_bytes(records))
    write_json(out / "corpus.json", manifest(records))


def check(out, wrong):
    records = corpus()
    expected_inputs = input_bytes(records)
    if any((out / name).read_bytes() != expected_inputs for name in ("inputs.bin", "uploaded-inputs.bin")):
        raise ValueError("original or uploaded input mismatch")
    if json.loads((out / "corpus.json").read_text()) != manifest(records):
        raise ValueError("corpus manifest changed")
    validate_native(json.loads((out / "native.json").read_text()), records)
    data = (out / "results.bin").read_bytes()
    result = examine(records, data, wrong)
    result["results_sha256"] = hashlib.sha256(data).hexdigest()
    result["inputs_sha256"] = hashlib.sha256(expected_inputs).hexdigest()
    if wrong:
        baseline = examine(records, data)
        result["control_valid"] = (baseline["integrity_and_exact_controls_pass"]
                                   and result["exact_control_mismatches"] == 1
                                   and result["identity_errors"] == result["inactive_lane_errors"] == 0)
    write_json(out / ("wrong-expected.json" if wrong else "check.json"), result)
    if wrong:
        return 1 if result["control_valid"] else 3
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
        if not 0 < len(data) <= 8 * 1024 * 1024:
            raise ValueError("empty or oversized artifact")
        if not public:
            for path in (root, home):
                if len(path) > 1 and any(path.encode(encoding) in data for encoding in ("utf-8", "utf-16le", "utf-16be")):
                    raise ValueError("nonpublic local path in artifact")


def selftest():
    records = corpus()
    assert INPUT.size == 64 and SAMPLE.size == 80
    assert len(input_bytes(records)) == len(records) * INPUT.size
    assert records == corpus() and set(r["operation"] for r in records) == set(range(5))
    synthetic = []
    for index, row in enumerate(records):
        expected = exact_control(row)
        values = (expected if expected is not None else 0x3EC00000, 0, 0, 0)
        if row["operation"] == 1:
            values = (0x3EC00000,) * 4
        synthetic.append(SAMPLE.pack(row["operation"], index, row["tag"], stamp(index, row),
                                     *input_words(row)[4:], *values))
    data = b"".join(synthetic)
    assert examine(records, data)["integrity_and_exact_controls_pass"]
    wrong = examine(records, data, True)
    assert not wrong["integrity_and_exact_controls_pass"] and wrong["exact_control_mismatches"] == 1
    broken = bytearray(data)
    broken[4] ^= 1
    assert examine(records, broken)["identity_errors"] == 1
    assert examine(records, bytes([0xA5]) * len(data))["identity_errors"] == len(records)
    try:
        examine(records, data[:-1])
    except ValueError:
        pass
    else:
        raise AssertionError("truncated result accepted")
    fake = {key: True for key in FLAGS}
    fake.update(schema=1, api="Metal compute", architecture="arm64", availability="available_and_executed",
                phase="completed", exit_code=0, error=None, command_status=4, threads=len(records),
                input_bytes=len(records)*INPUT.size, result_bytes=len(records)*SAMPLE.size,
                operation_records=[sum(r["operation"] == op for r in records) for op in range(5)],
                os_major=26, device_name="synthetic unit test; not native evidence")
    validate_native(fake, records)
    for key, bad in (("command_status", 3), ("pipeline_created", 1), ("readback_copied_after_completion", False),
                     ("output_guard_unchanged", False), ("operation_records", [0]*5)):
        try:
            validate_native({**fake, key: bad}, records)
        except ValueError:
            pass
        else:
            raise AssertionError("malformed native metadata accepted")
    for row in records:
        if row["operation"] == 1:
            for lane, scalar in enumerate(row["scalar_links"]):
                assert (row["a"][lane], row["b"][lane]) == (records[scalar]["a"][0], records[scalar]["b"][0])
    info = manifest(records)
    print(json.dumps({"status": "LOCAL_SYNTHETIC_CHECKS_ONLY_NOT_NATIVE", "records": len(records),
                      "operation_records": info["operation_records"], "result_components": info["result_components"],
                      "exact_finite_maximum_controls": info["exact_finite_maximum_controls"],
                      "wrong_expectation_rejected": True, "identity_poison_truncation_metadata_controls_rejected": True,
                      "scalar_vector_pairing_checked": True}, sort_keys=True))


def main():
    if sys.argv[1:] == ["selftest"]:
        selftest()
        return 0
    if len(sys.argv) != 3:
        raise ValueError("usage: selftest | generate/check/wrong-expected/audit DIRECTORY")
    command, out = sys.argv[1], Path(sys.argv[2])
    if command == "generate":
        generate(out)
        return 0
    if command == "check":
        return check(out, False)
    if command == "wrong-expected":
        return check(out, True)
    if command == "audit":
        audit(out)
        return 0
    raise ValueError("unknown command")


if __name__ == "__main__":
    try:
        status = main()
    except (ValueError, OSError, AssertionError, KeyError, TypeError, struct.error):
        print("math cohort refused: integrity, exact control or privacy failure", file=sys.stderr)
        status = 3
    sys.exit(status)
