#!/usr/bin/env python3
"""Generic inputs and exact integrity/anchor checks, not a filter implementation.

All non-anchor native arithmetic is recorded without an expected-result model.
The deliberate wrong-expectation check must fail while retaining its diagnostic.
"""
import hashlib
import json
import math
from pathlib import Path
import struct
import sys

OFFSETS = [(-1, 0), (-1, 1), (0, -1), (0, 0), (0, 1), (0, 2),
           (1, -1), (1, 0), (1, 1), (1, 2), (2, 0), (2, 1),
           (-8, -8), (7, 7), (-8, 7), (7, -8)]
GEOMETRIES = [(3, 5), (5, 3)]
FORMATS = [("RGBA8Unorm", 70, 4), ("BGRA8Unorm", 80, 4), ("RGBA16Float", 115, 8)]
STATES = [("nearest", "edge", True), ("linear", "edge", True),
          ("nearest", "edge", False), ("linear", "edge", False),
          ("nearest", "zero", True), ("linear", "zero", True),
          ("nearest", "zero", False), ("linear", "zero", False),
          ("nearest", "repeat", True), ("linear", "repeat", True),
          ("nearest", "mirror", True), ("linear", "mirror", True)]

def f32(x):
    return struct.unpack("<f", struct.pack("<f", x))[0]

def bits(x):
    return struct.unpack("<I", struct.pack("<f", x))[0]

def dump(path, obj):
    path.write_text(json.dumps(obj, sort_keys=True, indent=2) + "\n", encoding="utf-8")

def cases(width, height):
    points = []
    def pixel(label, x, y):
        p = [f32(x), f32(y)]
        points.append((label, [f32(p[0] / width), f32(p[1] / height)], p))
    def normalized(label, x, y):
        n = [f32(x), f32(y)]
        points.append((label, n, [f32(n[0] * width), f32(n[1] * height)]))
    pixel("exact_anchor", 0.5, 0.5)
    pixel("last_center", width - 0.5, height - 0.5)
    for x in [-1.375, -0.75, -0.25, 0, 0.25, 0.5, 0.75, 1, 1.5,
              width - 0.25, width, width + 0.25, width + 0.75]:
        for y in [-0.25, 0.5, height + 0.25]:
            pixel("edge_crossing", x, y)
    normalized("precision_boundary", struct.unpack("<f", struct.pack("<I", 0x3e2caaa9))[0], 0.5)
    for x, y in [(-1.3125, 1.125), (-0.03125, 0.1875), (0.0, 0.0),
                 (0.5, 0.5), (0.9375, 0.0625), (1, 1), (1.03125, -0.125)]:
        normalized("normalized_crossing", x, y)
    seed = 0x9e3779b9
    for _ in range(24):
        pair = []
        for _ in range(2):
            seed = (1664525 * seed + 1013904223) & 0xffffffff
            pair.append(f32((seed >> 8) / float(1 << 24) * 5.0 - 2.0))
        normalized("finite_fraction", *pair)
    out = []
    for oi, (ox, oy) in enumerate(OFFSETS):
        local = list(points)
        for label, x, y in [
            ("offset_moves_to_first_center", 0.5 - ox, 0.5 - oy),
            ("offset_moves_to_last_center", width - 0.5 - ox, height - 0.5 - oy),
            ("offset_crosses_lower_edge", -0.25 - ox, 0.75 - oy),
            ("offset_crosses_upper_edge", width + 0.25 - ox, height - 0.75 - oy),
        ]:
            p = [f32(x), f32(y)]
            local.append((label, [f32(p[0] / width), f32(p[1] / height)], p))
        for label, norm, pixel_uv in local:
            out.append({"id": len(out), "label": label, "offset_case": oi,
                        "offset": [ox, oy], "normalized_bits": list(map(bits, norm)),
                        "pixel_bits": list(map(bits, pixel_uv)),
                        "exact_anchor": label == "exact_anchor" and (ox, oy) == (0, 0)})
    return out

def image_data(width, height, token):
    out = bytearray()
    finite = [0.0, -0.0, 2.0**-24, -2.0**-24, 2.0**-14, -1.0,
              0.5, 1.0, 2.0, 10.0, 31.9375, 65504.0, -32.5]
    for y in range(height):
        for x in range(width):
            if token == 115:
                rgba = [1.0 if x == width - 1 else 0.0] + [finite[(x * 3 + y * 7 + c * 5) % len(finite)] for c in range(1, 4)]
                if (x, y) == (0, 0):
                    rgba = [2.0, -1.0, 0.5, 1.0]
                assert all(math.isfinite(v) for v in rgba)
                out.extend(struct.pack("<4e", *rgba))
            else:
                rgba = [255 if x == width - 1 else 0,
                        (x * 41 + y * 73 + 19) % 256,
                        (x * 89 + y * 31 + 53) % 256, (x * 23 + y * 47 + 137) % 256]
                if (x, y) == (0, 0):
                    rgba = [0, 255, 0, 255]
                if token == 80:
                    rgba[0], rgba[2] = rgba[2], rgba[0]
                out.extend(bytes(rgba))
    return out

def generate(out):
    out.mkdir(parents=True, exist_ok=True)
    corpus = {"version": 1, "input_stride": 32, "output_stride": 32,
              "stage": "compute", "lod": "explicit positive zero", "offsets": OFFSETS,
              "geometries": [], "images": [], "samplers": [], "runs": [],
              "non_anchor_expectations": "not supplied; native observations only"}
    inputs, images = bytearray(), bytearray()
    for gi, (width, height) in enumerate(GEOMETRIES):
        rows = cases(width, height)
        corpus["geometries"].append({"width": width, "height": height,
            "input_offset": len(inputs), "case_count": len(rows), "cases": rows})
        for c in rows:
            inputs.extend(struct.pack("<8I", *c["normalized_bits"], *c["pixel_bits"], c["offset_case"], c["id"], 0, 0))
        for name, token, bpp in FORMATS:
            data = image_data(width, height, token)
            corpus["images"].append({"geometry": gi, "width": width, "height": height,
                "name": name, "pixel_format": token, "pitch": width * bpp,
                "data_offset": len(images), "data_bytes": len(data), "mip_levels": 1,
                "sample_count": 1, "array_length": 1,
                "anchor_bits": list(map(bits, [2.0, -1.0, 0.5, 1.0] if token == 115 else [0.0, 1.0, 0.0, 1.0]))})
            images.extend(data)
    for linear, address, normalized in STATES:
        corpus["samplers"].append({"filter": linear, "address": address, "normalized": normalized,
            "min_equals_mag": True, "mip_filter": "not_mipmapped", "max_anisotropy": 1})
    for image in range(len(corpus["images"])):
        for sampler in range(len(STATES)):
            for precision in [32, 16]:
                corpus["runs"].append({"id": len(corpus["runs"]), "image": image, "sampler": sampler, "return_precision": precision})
    counts = {g["case_count"] for g in corpus["geometries"]}
    assert len(counts) == 1
    corpus["case_count"] = counts.pop()
    corpus["input_sha256"] = hashlib.sha256(inputs).hexdigest()
    corpus["image_sha256"] = hashlib.sha256(images).hexdigest()
    (out / "inputs.bin").write_bytes(inputs)
    (out / "image-data.bin").write_bytes(images)
    dump(out / "corpus.json", corpus)

def check(out, wrong=False):
    corpus = json.loads((out / "corpus.json").read_text())
    native = json.loads((out / "native.json").read_text())
    errors = []
    for key in ["pipeline_created", "single_mip_images_created", "samplers_created",
                "input_images_read_back_equal", "wait_until_completed_called", "command_completed",
                "readback_copied_after_successful_wait"]:
        if native.get(key) is not True:
            errors.append({"flag": key, "actual": native.get(key)})
    if native.get("phase") != "completed" or native.get("error") is not None:
        errors.append({"phase": native.get("phase"), "error": native.get("error")})
    source = (out / "image-data.bin").read_bytes()
    if (out / "uploaded-images.bin").read_bytes() != source:
        errors.append({"image_upload_readback": "different"})
    if hashlib.sha256(source).hexdigest() != corpus["image_sha256"]:
        errors.append({"source_image_hash": "different"})
    if hashlib.sha256((out / "inputs.bin").read_bytes()).hexdigest() != corpus["input_sha256"]:
        errors.append({"input_hash": "different"})
    raw = (out / "results.bin").read_bytes()
    count = corpus["case_count"]
    expected_bytes = len(corpus["runs"]) * count * 32
    if len(raw) != expected_bytes:
        errors.append({"readback_bytes": len(raw), "expected": expected_bytes})
    anchors, stamps, wrong_injected = 0, 0, False
    if len(raw) == expected_bytes:
        for run in corpus["runs"]:
            image = corpus["images"][run["image"]]
            geometry = corpus["geometries"][image["geometry"]]
            for c in geometry["cases"]:
                at = (run["id"] * count + c["id"]) * 32
                row = struct.unpack_from("<8I", raw, at)
                tag = [0x53504c00 ^ run["id"], c["id"], image["width"] | (image["height"] << 16), 1]
                if list(row[:4]) != tag:
                    if len(errors) < 20:
                        errors.append({"run": run["id"], "case": c["id"], "stamp": list(row[:4]), "expected_stamp": tag})
                else:
                    stamps += 1
                if c["exact_anchor"]:
                    expected = list(image["anchor_bits"])
                    if wrong and not wrong_injected:
                        expected[0] ^= 1
                        wrong_injected = True
                    anchors += 1
                    if list(row[4:]) != expected:
                        errors.append({"run": run["id"], "case": c["id"], "anchor": list(row[4:]), "expected": expected})
    result = {"pass": not errors, "errors": errors, "verified_stamps": stamps,
              "expected_stamps": len(corpus["runs"]) * count, "anchor_controls": anchors,
              "wrong_expected_injected": wrong_injected,
              "non_anchor_numeric_results": "not judged; raw native observations retained"}
    dump(out / ("wrong-expected.json" if wrong else "check.json"), result)
    return 0 if not errors else 1

def selftest():
    assert len(OFFSETS) == 16 and len(set(OFFSETS)) == 16
    assert all(-8 <= x <= 7 and -8 <= y <= 7 for x, y in OFFSETS)
    for w, h in GEOMETRIES:
        rows = cases(w, h)
        assert sum(c["exact_anchor"] for c in rows) == 1
        assert any(c["normalized_bits"][0] == 0x3e2caaa9 and c["offset"] == [1, 0] for c in rows)
        assert any(c["label"] == "offset_moves_to_first_center" and c["offset"] == [7, 7] for c in rows)
        for _, token, bpp in FORMATS:
            assert len(image_data(w, h, token)) == w * h * bpp
    return 0

if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "selftest":
        raise SystemExit(selftest())
    out = Path(sys.argv[2])
    if mode == "generate":
        generate(out)
    elif mode in ["check", "wrong-expected"]:
        raise SystemExit(check(out, mode == "wrong-expected"))
    else:
        raise SystemExit("unknown operation")
