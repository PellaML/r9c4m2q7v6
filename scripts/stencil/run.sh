#!/bin/bash
# Generic synthetic Metal probe: one self-contained bounded runner and verifier.
# Local toolchains and Python standard library only; no installs or downloads.
set -euo pipefail
umask 077
here="$(cd "$(dirname "$0")" && pwd)"
exec python3 -B - "$here" "$@" <<'PY_STENCIL_INTERNAL'
#!/usr/bin/env python3
"""Independent exact-byte audit. This is a validator, NOT a CPU stencil oracle."""
import collections
import hashlib
import json
import re
import struct
import sys
from pathlib import Path

sys.dont_write_bytecode = True
POISON = 0xCD
MAGIC = 0x53543847
MAX_FILE = 1_048_576
BLACK = bytes((0, 0, 0, 255))
RED = bytes((255, 0, 0, 255))
GREEN = bytes((0, 255, 0, 255))
BLUE = bytes((0, 0, 255, 255))


class Invalid(ValueError):
    pass


def require(ok, message):
    if not ok:
        raise Invalid(message)


def file_bytes(root, name):
    require(isinstance(name, str) and re.fullmatch(r"[a-z0-9][a-z0-9.-]*\.(bin|json|txt)", name),
            "invalid_artifact_name")
    p = root / name
    require(p.is_file() and not p.is_symlink() and p.stat().st_size <= MAX_FILE, "invalid_artifact_file")
    return p.read_bytes()


def read_json(root, name):
    return json.loads(file_bytes(root, name))


def unpack_plane(raw, plane):
    """Return unchanged payload bytes and independently count every guard byte."""
    w, h, bpp = (plane[k] for k in ("width", "height", "bytes_per_pixel"))
    require(type(w) is int and type(h) is int and type(bpp) is int, "plane_dimension_types")
    require(1 <= w <= 16 and 1 <= h <= 16 and bpp in (1, 4, 16), "plane_dimension_bounds")
    pitch = ((w * bpp + 255) // 256) * 256
    require(plane["offset"] == 256 and plane["bytes_per_row"] == pitch and
            plane["bytes_per_image"] == pitch * h and plane["suffix_bytes"] == 256 and
            plane["poison_guard_byte"] == POISON and plane["length"] == 512 + pitch * h and
            len(raw) == plane["length"], "plane_layout")
    require(plane["storage_name"] == "shared" and plane["storage"] == 0, "plane_not_shared")
    payload = bytearray()
    bad = 0
    for i, value in enumerate(raw):
        local = i - 256
        active = 0 <= local < pitch * h and local % pitch < w * bpp
        if active:
            payload.append(value)
        elif value != POISON:
            bad += 1
    require(plane["bad_guard_bytes"] == bad, "native_guard_count_disagrees")
    return bytes(payload), bad


def ordinary_errors(planes, tag, spec=None, ramp=False):
    """The SAME validator is used for positive and actual GPU negative captures."""
    errors = []
    for name in ("color", "query", "stencil", "markers"):
        if name in planes and planes[name][1]:
            errors.append("guard-" + name)
    m = planes["markers"][0]
    require(len(m) == 64, "marker_length")
    for phase in range(4):
        if struct.unpack_from("<4I", m, phase * 16) != (MAGIC, phase, tag, (~phase) & 0xFFFFFFFF):
            errors.append(f"marker-phase-{phase}")
    color = planes["color"][0]
    require(len(color) in (256, 1024), "color_length")
    if any(color[i + 3] != 255 or color[i:i + 4] == b"\xcd" * 4 for i in range(0, len(color), 4)):
        errors.append("color-unwritten-or-invalid-alpha")
    measured = None
    if "query" in planes:
        q = planes["query"][0]
        require(len(q) == len(color), "query_length")
        if any(q[i + 1] != 255 - q[i] or q[i + 2] != 0x53 or q[i + 3] != 255
               for i in range(0, len(q), 4)):
            errors.append("query-unwritten-or-invalid-signature")
        else:
            measured = q[::4]
    if "stencil" in planes:
        s = planes["stencil"][0]
        require(len(s) * 4 == len(color), "stencil_length")
        if measured is not None and measured != s:
            errors.append("direct-query-disagreement")
        if measured is None and "query" not in planes:
            measured = s
    if ramp:
        if measured != bytes(range(256)):
            errors.append("ramp-stencil-anchor")
        if color != BLACK * 256:
            errors.append("ramp-color-anchor")
    if spec:
        if spec.get("anchorColor") is not None and color != bytes(spec["anchorColor"]) * (len(color) // 4):
            errors.append("anchor-color")
        if spec.get("anchorStencil") is not None and measured != bytes((spec["anchorStencil"],)) * (len(color) // 4):
            errors.append("anchor-stencil")
    return errors, color, measured


def command_receipts(report, expected_labels):
    commands = report["commands"]
    require([c["label"] for c in commands] == expected_labels, "command_labels_or_count")
    for c in commands:
        require(c["committed"] is True and c["wait_until_completed_returned"] is True and
                c["completed"] is True and c["status"] == 4 and c["error_present"] is False and
                "error" not in c, "command_not_cleanly_completed")


def check_capture_files(root, report, outputs, prefix, required, size, used):
    require(set(outputs) == required, "output_plane_set")
    result = {}
    for name, plane in outputs.items():
        file_name = prefix + "." + name + ".bin"
        require(plane["file"] == file_name and file_name not in used, "output_name_or_reuse")
        require((plane["width"], plane["height"], plane["bytes_per_pixel"]) ==
                ((4, 1, 16) if name == "markers" else (size, size, 1 if name == "stencil" else 4)),
                "capture_plane_dimensions")
        raw = file_bytes(root, file_name)
        require(report["output_sha256"].get(file_name) == hashlib.sha256(raw).hexdigest(), "output_hash_mismatch")
        used.add(file_name)
        result[name] = unpack_plane(raw, plane)
    return result


def texture_contract(t, format_value, size):
    require(t["pixel_format"] == format_value and t["storage"] == 2 and t["usage"] == 4 and
            t["type"] == 2 and t["width"] == size and t["height"] == size and t["depth"] == 1 and
            t["array_length"] == 1 and t["mips"] == 1 and t["samples"] == 1 and
            t["framebuffer_only"] is False and t["buffer_backed"] is False, "actual_texture_contract")


def histogram(data, stride=1):
    if data is None:
        return None
    return {k.hex(): v for k, v in sorted(collections.Counter(
        data[i:i + stride] for i in range(0, len(data), stride)).items())}


def verify(root):
    manifest = read_json(root, "cases.json")
    native = read_json(root, "native.json")
    decision = read_json(root, "readout-decision.json")
    require(native["schema"] == 1 and native["status"] == "OBSERVATIONS_COMPLETE" and
            native["exit_code"] == 0 and native["negative_controls_complete"] is True, "native_incomplete")
    require(manifest["caseCount"] == 45 and len(manifest["cases"]) == 45, "case_count")
    require(native["case_manifest_sha256"] == hashlib.sha256(file_bytes(root, "cases.json")).hexdigest(), "manifest_hash")
    require(native["vertex_sha256"] == hashlib.sha256(file_bytes(root, "vertices.bin")).hexdigest() and
            native["vertex_input_preserved"] is True, "vertex_input_hash_or_mutation")
    direct = decision["direct_enabled"]
    require(type(direct) is bool and native["direct_blit_enabled"] is direct, "readout_decision")
    require(decision["mode"] == ("direct-plus-sweep" if direct else "sweep-only"), "readout_mode")
    require(native["contract"]["cpu_renderer"] is False and native["contract"]["tolerance"] == 0 and
            native["contract"]["depth_attachment"] == "nil" and
            native["contract"]["shader_depth_output"] is False and
            native["contract"]["shader_stencil_output"] is False and
            native["contract"]["early_fragment_tests_requested"] is False, "native_contract")
    require(isinstance(native["device"]["name"], str) and native["device"]["name"], "device_identity")
    if decision.get("require_paravirtual"):
        require("paravirtual" in native["device"]["name"].lower(), "primary_gpu_not_paravirtual")
    specs = {c["name"]: c for c in manifest["cases"]}
    require(len(specs) == 45, "duplicate_case")
    expected_names = ["query-calibration", *specs, "negative-skip-draw", "negative-skip-marker"]
    obs = native["observations"]
    require([o["name"] for o in obs] == expected_names, "observation_names_or_count")
    labels = [name + suffix for name in expected_names for suffix in ("-test", "-readout")]
    command_receipts(native, labels)
    require(native["command_buffer_count"] == len(labels) == 96 and
            11000 <= native["draw_count"] <= 14000, "native_work_bounds")
    all_used, native_used, rows, captured = set(), set(), [], {}
    for index, o in enumerate(obs):
        name = o["name"]
        negative = name.startswith("negative-")
        calibration = name == "query-calibration"
        spec = specs["always-replace-1"] if negative else specs.get(name)
        stored = calibration or spec["store"] == "store"
        size = 16 if calibration else 8
        tag = (100 if name == "negative-skip-draw" else 101) if negative else index + 1
        require(o["tag"] == tag and o["calibration"] is calibration, "capture_tag")
        expected_mutation = name.removeprefix("negative-") if negative else "none"
        require(o["mutation"] == expected_mutation, "gpu_mutation_receipt")
        required = {"color", "markers"} | ({"query"} if stored else set()) | ({"stencil"} if stored and direct else set())
        planes = check_capture_files(root, native, o["outputs"], name, required, size, native_used)
        texture_contract(o["actual_textures"]["color"], 73, size)
        texture_contract(o["actual_textures"]["stencil"], 253, size)
        p = o["render_pass_descriptor_getters"]
        require(p["depth_texture_nil"] is True and p["color_load"] == 2 and p["color_store"] == 1 and
                p["clear_color"] == [0, 0, 0, 255], "test_pass_contract")
        if not calibration:
            require(p["stencil_load"] == (2 if spec["load"] == "clear" else 0) and
                    p["stencil_store"] == (1 if stored else 0) and o["clear_stencil_call_u32"] == spec["clear"],
                    "test_pass_actions_or_clear_submission")
            require(len(o["draw_calls"]) == len(spec["draws"]), "draw_receipt_count")
            for i, (actual, requested) in enumerate(zip(o["draw_calls"], spec["draws"])):
                require(actual["front_reference_call_u32"] == requested["frontRef"] and
                        actual["back_reference_call_u32"] == requested["backRef"] and
                        actual["split_reference_call"] == requested["splitRef"] and
                        actual["draw_submitted"] == (not (name == "negative-skip-draw" and i == 0)),
                        "reference_narrowed_or_draw_not_submitted")
                if requested["mode"] == "enabled":
                    for side in ("front", "back"):
                        require(actual["submitted_masks_u32"][side] ==
                                [requested[side]["readMask"], requested[side]["writeMask"]],
                                "descriptor_mask_narrowed_before_submission")
                        # Native property getters are observations too. Do not
                        # pre-assume whether a descriptor retains high bits.
                        for key in ("read_mask_u32", "write_mask_u32"):
                            require(0 <= actual["descriptor_getters"][side][key] <= 0xFFFFFFFF,
                                    "descriptor_getter_u32_range")
        if stored:
            require(o["post_pass_stencil_access"] is True and o["gpu_query"]["draw_count"] == 256 and
                    o["gpu_query"]["references"] == [0, 255], "query_missing")
            qp = o["gpu_query"]["pass_descriptor_getters"]
            require(qp["stencil_load"] == 1 and qp["stencil_store"] == 1 and
                    qp["depth_texture_nil"] is True and qp["clear_color"] == [0, 0, 0, 0], "query_pass_contract")
        else:
            require(o["post_pass_stencil_access"] is False and o["gpu_query"] is None and
                    o["stencil_publication"] == "NONE_DONTCARE_STORE", "dontcare_publication_forbidden")
        errors, color, stencil = ordinary_errors(planes, tag, spec, ramp=calibration)
        require(errors == o["ordinary_validation_errors"], "ordinary_validator_disagreement")
        if negative:
            expected = ["anchor-color", "anchor-stencil"] if name == "negative-skip-draw" else ["marker-phase-3"]
            require(errors == expected and o["status"] == "NEGATIVE_REJECTED" and
                    o["ordinary_validator_rejected"] is True and o["expected_rejection_errors"] == expected,
                    "actual_gpu_negative_was_not_rejected")
        else:
            require(not errors and o["status"] == "OBSERVED", "ordinary_capture_rejected")
        captured[name] = (color, stencil)
        rows.append({"name": name, "color_rgba8uint_histogram": histogram(color, 4),
                     "stencil_u8_histogram": histogram(stencil),
                     "stencil_origin": "GPU query colour R lane" if stored else "NOT_PUBLISHED_DONTCARE",
                     "validation_errors": errors})
    require(native_used == set(native["output_sha256"]), "undeclared_or_missing_native_output")
    require(native["raw_output_files"] == len(native_used) and native["raw_output_bytes"] ==
            sum((root / f).stat().st_size for f in native_used), "native_output_totals")
    all_used |= native_used

    blit = read_json(root, "blit.json")
    if direct:
        require(blit["status"] == "DIRECT_BLIT_EXACT" and blit["exit_code"] == 0 and
                decision["attempt_exit"] == 0, "direct_blit_not_proven")
        command_receipts(blit, ["direct-control-render", "direct-control-stencil-copy"])
        used = set()
        planes = check_capture_files(root, blit, blit["outputs"], "direct-control",
                                     {"color", "stencil", "markers"}, 16, used)
        errors, _, _ = ordinary_errors(planes, blit["tag"], ramp=True)
        require(not errors and not blit["validation_errors"] and used == set(blit["output_sha256"]), "direct_control_bytes")
        all_used |= used
    else:
        # Fallback cannot launder a completed-but-wrong blit into a success.
        require(blit.get("stage") == "before_direct_stencil_blit" and
                blit.get("status") != "DIRECT_BLIT_EXACT" and decision["attempt_exit"] in (-6, 77, 134),
                "unexplained_direct_blit_failure")
        command_receipts({"commands": blit["commands"][:1]}, ["direct-control-render"])
        if decision["attempt_exit"] == 77:
            require(len(blit["commands"]) == 2 and
                    blit["commands"][1]["label"] == "direct-control-stencil-copy" and
                    blit["commands"][1]["completed"] is False, "fallback_copy_did_not_fail")
        else:
            require(len(blit["commands"]) == 1, "completed_direct_copy_cannot_fall_back")
        require(not blit.get("outputs") and not blit.get("output_sha256"), "partial_direct_output_forbidden")
    require({p.name for p in root.glob("*.bin")} == all_used | {"vertices.bin"}, "unlisted_binary_or_dontcare_stencil_file")

    # Exact relational controls on fully initialized low references, not a general
    # software stencil implementation. Exploratory narrowing/discard state remains data.
    write_then = captured["overlap-write-then-test"]
    test_then = captured["overlap-test-then-write"]
    expected_color = (BLACK * 4 + BLUE * 4) * 8
    expected_stencil = bytes([1] * 4 + [0] * 4) * 8
    require(write_then == (expected_color, expected_stencil), "color_mask_zero_state_write_or_order")
    require(test_then == (BLUE * 64, expected_stencil), "reverse_draw_order")
    require(captured["dontcare-load-initialized"] == write_then, "initialized_dontcare_load_differs")
    for name in ("dontcare-store", "dontcare-load-store"):
        require(captured[name] == (write_then[0], None), "dontcare_color_or_no_publication")
    require(captured["opposed-identical-faces"] == ((RED * 4 + GREEN * 4) * 8, bytes([1]) * 64),
            "identical_faces_on_opposed_winding")
    require(captured["discard-none"] == captured["always-replace-1"], "discard_reference_case")
    require(captured["discard-all"][0] == BLACK * 64 and
            captured["discard-left"][0] == (BLACK * 4 + RED * 4) * 8, "discard_color_effect")
    discard = []
    for name in ("discard-none", "discard-all", "discard-left", "discard-stencil-fail"):
        stencil = captured[name][1]
        discard.append({"name": name, "actual_gpu_query_stencil_rows_hex":
                        [stencil[i:i + 8].hex() for i in range(0, 64, 8)]})
    return {"schema": 1, "status": "EXACT_EVIDENCE_VALIDATED", "native_measurement": True,
            "device": native["device"], "readout_decision": decision, "cases": 45,
            "captures_including_calibration_and_negatives": 48, "exact_comparisons": True,
            "tolerance": 0, "cpu_rendered_result_bytes": 0,
            "gpu_negatives_rejected": ["skip-draw", "skip-marker"],
            "dontcare_post_store_stencil_publication": False,
            "observations": rows, "discard_state_observations": discard,
            "interpretation": "Only actual bytes from this run. U32 narrowing and discard/stencil outcomes are observations, not assumed rules."}


def write_verification(root):
    try:
        result = verify(root)
        code = 0
    except (Invalid, KeyError, ValueError, TypeError, OSError) as e:
        # Only our fixed validation codes are public error text.
        result = {"schema": 1, "status": "FAILED_NOT_PASS", "native_measurement": False,
                  "reason": str(e) if isinstance(e, Invalid) else type(e).__name__}
        code = 3
    text = json.dumps(result, indent=2, sort_keys=True) + "\n"
    require(len(text.encode()) <= MAX_FILE, "verification_report_budget")
    with (root / "verification.json").open("x", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("verification_exit=" + str(code))
    return code



#!/usr/bin/env python3
"""Bounded local orchestration; no installs, downloads, credentials or CPU renderer."""
import datetime
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.dont_write_bytecode = True
SOURCE = None  # Set only from this wrapper's own directory, not caller data.
SOURCES = [
    ".github/workflows/stencil.yml", "scripts/stencil/cases.rs",
    "scripts/stencil/probe.swift", "scripts/stencil/probe.metal", "scripts/stencil/run.sh",
]
MAX_FILES = 256
MAX_FILE = 1_048_576
MAX_TOTAL = 8_388_608
MAX_LOG = 65_536
TOTAL_SECONDS = 900
ROOT_FILES = {"command-exit.json", "run-receipt.json", "source-sha256.txt", "sha256.txt"}


class Stop(RuntimeError):
    pass


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def write_json(path, value):
    raw = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
    if len(raw) > MAX_FILE:
        raise Stop("json_file_budget")
    path.write_bytes(raw)


def regular_files(root):
    if root.is_symlink() or not root.is_dir() or (root / "results").is_symlink() or not (root / "results").is_dir():
        raise Stop("artifact_root_shape")
    files = []
    for p in root.rglob("*"):
        if p.is_symlink():
            raise Stop("artifact_symlink")
        if p.is_dir():
            if p != root / "results":
                raise Stop("unexpected_artifact_directory")
            continue
        if not p.is_file():
            raise Stop("artifact_special_file")
        relative = p.relative_to(root).as_posix()
        if relative not in ROOT_FILES and not re.fullmatch(r"results/[a-z0-9][a-z0-9.-]*\.(bin|json|txt)", relative):
            raise Stop("artifact_allowlist")
        if p.stat().st_size > MAX_FILE:
            raise Stop("artifact_per_file_budget")
        files.append(p)
        if len(files) > MAX_FILES:
            raise Stop("artifact_file_count_budget")
    if not 5 <= len(files) <= MAX_FILES or sum(p.stat().st_size for p in files) > MAX_TOTAL:
        raise Stop("artifact_total_budget")
    if not ROOT_FILES <= {p.name for p in files if p.parent == root}:
        raise Stop("artifact_receipt_missing")
    return sorted(files)


def seal(root):
    # Include every artifact except the self-referential hash manifest.
    (root / "sha256.txt").write_text("", encoding="ascii")
    files = regular_files(root)
    lines = [hashlib.sha256(p.read_bytes()).hexdigest() + "  " + p.relative_to(root).as_posix()
             for p in files if p.name != "sha256.txt"]
    (root / "sha256.txt").write_text("\n".join(lines) + "\n", encoding="ascii", newline="\n")
    return check_artifacts(root)


def check_artifacts(root):
    files = regular_files(root)
    wanted = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in files if p.name != "sha256.txt"}
    got = {}
    for line in (root / "sha256.txt").read_text(encoding="ascii").splitlines():
        if not re.fullmatch(r"[0-9a-f]{64}  [a-zA-Z0-9./-]+", line):
            raise Stop("artifact_hash_manifest_syntax")
        digest, name = line.split("  ", 1)
        if name in got:
            raise Stop("artifact_duplicate_hash_entry")
        got[name] = digest
    if got != wanted:
        raise Stop("artifact_hash_or_file_set_mismatch")
    return {"files": len(files), "bytes": sum(p.stat().st_size for p in files),
            "max_files": MAX_FILES, "max_file_bytes": MAX_FILE, "max_total_bytes": MAX_TOTAL}


def choose_readout(code, blit):
    """Only a bounded, isolated unsupported/aborted direct copy can fall back."""
    if code == 0 and blit.get("status") == "DIRECT_BLIT_EXACT" and blit.get("exit_code") == 0:
        return True, "direct-control-exact-all-256"
    render = blit.get("commands", [])
    ready = blit.get("stage") == "before_direct_stencil_blit" and len(render) >= 1 and all(
        render[0].get(k) is True for k in ("committed", "wait_until_completed_returned", "completed"))
    ready = ready and render[0].get("label") == "direct-control-render" and render[0].get("error_present") is False
    if code == 77:
        ready = ready and len(render) == 2 and render[1].get("label") == "direct-control-stencil-copy" and render[1].get("completed") is False
    else:
        # A completed direct command followed by a later abort is not an
        # unsupported blit. It is an unexplained control failure.
        ready = ready and len(render) == 1
    if code in (-6, 77, 134) and ready and not blit.get("outputs") and not blit.get("output_sha256"):
        return False, ("direct-command-unavailable" if code == 77 else "isolated-direct-validation-abort-support-unproven")
    raise Stop("direct_control_failure_not_eligible_for_fallback")


class Runner:
    def __init__(self, out, source=None):
        self.out = out.resolve()
        self.source = (SOURCE if source is None else source).resolve()
        self.results = self.out / "results"
        self.build = self.out / ".build"
        self.deadline = time.monotonic() + TOTAL_SECONDS
        self.status = {key: 99 for key in (
            "source_hash", "metadata", "rust_test_build", "rust_tests", "rust_build", "generation",
            "swift_build", "blit_attempt", "probe", "verification", "packaging")}
        self.receipt = {"schema": 1, "started_utc": utc(), "status": "FAILED_NOT_PASS",
                        "platform": sys.platform, "commands": [], "not_run_sentinel": 99,
                        "bounds": {"total_seconds": TOTAL_SECONDS, "log_bytes_per_stream": MAX_LOG,
                                   "files": MAX_FILES, "file_bytes": MAX_FILE, "total_bytes": MAX_TOTAL},
                        "environment_overrides": {"MTL_DEBUG_LAYER": "1", "module_caches": "OUTPUT/.build/module-cache"}}
        self.truncated = 0

    def save_receipt(self):
        write_json(self.out / "run-receipt.json", self.receipt)

    def command(self, label, argv, seconds, *, env=None):
        if not re.fullmatch(r"[a-z0-9-]+", label):
            raise Stop("command_label")
        budget = min(seconds, self.deadline - time.monotonic())
        if budget <= 0:
            raise Stop("total_time_budget")
        args = [str(a) for a in argv]
        display = [a.replace(str(self.source), "SOURCE").replace(str(self.out), "OUTPUT") for a in args]
        entry = {"label": label, "argv": display, "timeout_seconds": round(budget, 3),
                 "started_utc": utc(), "exit": 99, "timed_out": False}
        self.receipt["commands"].append(entry)
        self.save_receipt()
        started = time.monotonic()
        child_env = os.environ.copy()
        child_env.update({"MTL_DEBUG_LAYER": "1",
                          "SWIFT_MODULECACHE_PATH": str(self.build / "module-cache"),
                          "CLANG_MODULE_CACHE_PATH": str(self.build / "module-cache")})
        if env:
            child_env.update(env)
        streams = {name: {"kept": bytearray(), "seen": 0} for name in ("stdout", "stderr")}

        def drain(pipe, record):
            try:
                while True:
                    chunk = pipe.read(4096)
                    if not chunk:
                        break
                    record["seen"] += len(chunk)
                    room = max(0, MAX_LOG - len(record["kept"]))
                    record["kept"].extend(chunk[:room])
            finally:
                pipe.close()

        process = None
        try:
            process = subprocess.Popen(args, cwd=self.source, env=child_env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       start_new_session=True)
            threads = [threading.Thread(target=drain, args=(pipe, streams[name]), daemon=True)
                       for name, pipe in (("stdout", process.stdout), ("stderr", process.stderr))]
            for t in threads:
                t.start()
            try:
                code = process.wait(timeout=budget)
            except subprocess.TimeoutExpired:
                entry["timed_out"] = True
                # Only the new process group belonging to this one command.
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
                process.wait(timeout=10)
                code = 124
            for t in threads:
                t.join(timeout=5)
            if any(t.is_alive() for t in threads):
                raise Stop("command_log_drain_did_not_finish")
        except OSError as error:
            code = 127
            entry["spawn_error_type"] = type(error).__name__
        except BaseException:
            if process is not None and process.poll() is None:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
                process.wait(timeout=10)
            raise
        for name, stream in streams.items():
            file_name = f"{label}.{name}.txt"
            (self.results / file_name).write_bytes(stream["kept"])
            cut = stream["seen"] > MAX_LOG
            self.truncated += int(cut)
            entry[name] = {"file": "results/" + file_name, "seen_bytes": stream["seen"],
                           "retained_bytes": len(stream["kept"]), "truncated": cut}
        entry.update(exit=code, elapsed_seconds=round(time.monotonic() - started, 6), finished_utc=utc())
        self.save_receipt()
        return code

    def stage(self, key, argv, seconds):
        self.status[key] = self.command(key.replace("_", "-"), argv, seconds)
        if self.status[key] != 0:
            raise Stop("stage_failed_" + key)

    def run(self):
        lines = []
        for name in SOURCES:
            p = self.source / name
            if not p.is_file() or p.is_symlink() or p.stat().st_size > MAX_FILE:
                raise Stop("public_source_file_missing_or_invalid")
            lines.append(hashlib.sha256(p.read_bytes()).hexdigest() + "  " + name)
        (self.out / "source-sha256.txt").write_text("\n".join(lines) + "\n", encoding="ascii", newline="\n")
        self.status["source_hash"] = 0
        if sys.platform != "darwin":
            self.status["metadata"] = 3
            raise Stop("native_metal_requires_macos_no_cpu_fallback")
        self.status["metadata"] = 0
        for label, argv in (
            ("os", ["sw_vers"]), ("arch", ["uname", "-m"]), ("xcode", ["xcodebuild", "-version"]),
            ("sdk-version", ["xcrun", "--sdk", "macosx", "--show-sdk-version"]),
            ("sdk-build", ["xcrun", "--sdk", "macosx", "--show-sdk-build-version"]),
            ("sdk-path", ["xcrun", "--sdk", "macosx", "--show-sdk-path"]),
            ("swift", ["xcrun", "swiftc", "--version"]), ("rust", ["rustc", "-vV"]),
            ("python", [sys.executable, "--version"]),
        ):
            if self.command("meta-" + label, argv, 20) != 0:
                self.status["metadata"] = 3
                raise Stop("metadata_command_failed")
        sdk = (self.results / "meta-sdk-path.stdout.txt").read_text(encoding="utf-8").strip()
        if not sdk or "\n" in sdk or not Path(sdk).is_dir():
            raise Stop("selected_sdk_path_invalid")
        self.receipt["swift_sdk_selection"] = "explicit -sdk from meta-sdk-path; SDK version/build recorded separately"
        self.build.mkdir()
        script = self.source / "scripts" / "stencil"
        rust_tests, cases, probe = (self.build / f for f in ("cases-tests", "cases", "probe"))
        self.stage("rust_test_build", ["rustc", "--edition=2021", "-D", "warnings", "--test",
                                      script / "cases.rs", "-o", rust_tests], 120)
        self.stage("rust_tests", [rust_tests], 30)
        self.stage("rust_build", ["rustc", "--edition=2021", "-D", "warnings", "-O",
                                 script / "cases.rs", "-o", cases], 120)
        self.stage("generation", [cases, self.results], 20)
        self.stage("swift_build", ["xcrun", "--sdk", "macosx", "swiftc", "-swift-version", "5", "-O", "-sdk", sdk,
                                  "-module-cache-path", self.build / "module-cache",
                                  script / "probe.swift", "-o", probe], 180)
        # A process-level validation abort is isolated to this optional capability
        # attempt. A wrong completed direct copy is a HARD failure, not a fallback.
        code = self.command("blit-control", [probe, "blit-control", script / "probe.metal", self.results, "direct"], 90)
        self.status["blit_attempt"] = code
        try:
            path = self.results / "blit.json"
            if path.is_symlink() or path.stat().st_size > MAX_FILE:
                raise Stop("blit_receipt_shape")
            blit = json.loads(path.read_bytes())
        except (OSError, ValueError):
            raise Stop("direct_control_missing_checkpoint") from None
        direct, reason = choose_readout(code, blit)
        runner_label = os.environ.get("STENCIL_RUNNER_LABEL", "unspecified")
        if runner_label not in ("macos-26", "xcode-27", "unspecified"):
            raise Stop("unrecognized_primary_runner_label")
        decision = {"schema": 1, "direct_enabled": direct,
                    "mode": "direct-plus-sweep" if direct else "sweep-only", "reason": reason,
                    "attempt_exit": code, "attempt_stage": blit.get("stage"),
                    "attempt_status": blit.get("status"), "runner_label": runner_label,
                    "require_paravirtual": os.environ.get("STENCIL_REQUIRE_PARAVIRTUAL", "0") == "1",
                    "cpu_substitution": False, "native_support_claim": "exact control" if direct else "unproven/unavailable; GPU sweep required"}
        write_json(self.results / "readout-decision.json", decision)
        self.stage("probe", [probe, "observe", script / "probe.metal", self.results,
                             "direct" if direct else "sweep"], 300)
        self.stage("verification", ["/bin/bash", script / "run.sh", "--verify", self.results], 30)
        if self.truncated:
            raise Stop("truncated_log_not_success")
        self.receipt["status"] = "OBSERVATIONS_COMPLETE"

    def finish(self, code):
        # Exact child directory, resolved and checked before recursive deletion.
        if self.build.exists() or self.build.is_symlink():
            if self.build.is_symlink() or self.build.resolve().parent != self.out.resolve() or self.build.name != ".build":
                self.receipt["packaging_failure"] = "unsafe_build_cleanup_path"
                code = 3
            else:
                shutil.rmtree(self.build)
        if not (self.results / "native.json").exists():
            write_json(self.results / "native.json", {"schema": 1, "status": "NOT_RUN", "exit_code": 99})
        if not (self.results / "verification.json").exists():
            write_json(self.results / "verification.json", {"schema": 1, "status": "NOT_RUN", "native_measurement": False})
        if not (self.out / "source-sha256.txt").exists():
            (self.out / "source-sha256.txt").write_text("", encoding="ascii")
        self.receipt.update(finished_utc=utc(), truncated_streams=self.truncated)
        self.status["packaging"] = 0
        exits = {key + "_exit": value for key, value in self.status.items()}
        exits.update(schema=1, run_exit=code, not_run_sentinel=99, truncated_streams=self.truncated)
        if code:
            self.receipt["status"] = "FAILED_NOT_PASS"
        self.save_receipt()
        write_json(self.out / "command-exit.json", exits)
        try:
            # The final checker reports actual bytes without putting a
            # self-referential byte total inside the hashed receipt itself.
            seal(self.out)
        except (OSError, Stop, ValueError) as e:
            exits["packaging_exit"] = 3
            exits["run_exit"] = code = 3
            self.receipt["status"] = "FAILED_NOT_PASS"
            self.receipt["packaging_failure"] = str(e) if isinstance(e, Stop) else type(e).__name__
            self.save_receipt()
            write_json(self.out / "command-exit.json", exits)
            try:
                seal(self.out)
            except (OSError, Stop, ValueError):
                pass
        print(json.dumps(exits, sort_keys=True))
        return code


def main():
    global SOURCE
    if len(sys.argv) < 2:
        return 2
    # The shell wrapper supplies its resolved scripts/stencil directory. Remove
    # that internal argument before handling the bounded public command surface.
    SOURCE = Path(sys.argv.pop(1)).resolve().parents[1]
    if len(sys.argv) == 3 and sys.argv[1] == "--verify":
        return write_verification(Path(sys.argv[2]))
    if len(sys.argv) == 3 and sys.argv[1] == "--check-artifacts":
        try:
            print(json.dumps(check_artifacts(Path(sys.argv[2]))))
            return 0
        except (OSError, ValueError, Stop):
            print("artifact_check=FAILED_NOT_PASS")
            return 3
    if len(sys.argv) != 1:
        return 2
    os.umask(0o077)
    out = Path.cwd() / "out-stencil"
    if out.exists() or out.is_symlink():
        print("out-stencil_must_be_fresh", file=sys.stderr)
        return 2
    out.mkdir(mode=0o700)
    (out / "results").mkdir(mode=0o700)
    runner = Runner(out)
    code = 0
    try:
        runner.run()
    except (OSError, ValueError, Stop) as e:
        runner.receipt["failure"] = str(e) if isinstance(e, Stop) else type(e).__name__
        code = 3
    except KeyboardInterrupt:
        runner.receipt["failure"] = "interrupted"
        code = 130
    return runner.finish(code)



if __name__ == "__main__":
    sys.exit(main())

PY_STENCIL_INTERNAL
