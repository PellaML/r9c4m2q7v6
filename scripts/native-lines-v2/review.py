#!/usr/bin/env python3
"""Bounded source review, local runner, and exact artifact allowlist. No network API."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent
SOURCES = (
    ".gitignore", "Cargo.toml", "Cargo.lock", "build.rs",
    "native/metal_probe.h", "native/metal_probe.m", "native/probe.metal",
    "src/main.rs", "src/ffi.rs", "src/cases.rs", "src/check.rs", "src/json.rs", "src/extension.rs",
    "EXPERIMENT.json",
    "run.sh", "review.py", "native-lines.workflow.yml.template",
)
MANIFEST = "PUBLIC-MANIFEST.json"
ROOT_RESULTS = {"source.sha256", "toolchain.json", "host-tests.json", "source-verified.json", "exit.json"}
NATIVE_RESULTS = {"inputs.json", "run.json", "observations.jsonl", "readbacks.bin", "summary.json", "generator.json", "diagnostics.jsonl"}
EXPECTED_CASES = 962
EXPECTED_MEASUREMENTS = 2 * EXPECTED_CASES
MAX_BUILD_BYTES = 96 * 1024**2
MAX_RAW_BYTES = 75 * 1024**2
MAX_EXPORT_BYTES = 96 * 1024**2
PRIVATE_PATH = re.compile(rb"(?:(?<![A-Za-z0-9])[A-Za-z]:[\\/]|/(?:Users|home|private|Volumes)/|file:[/]{2})")


def need(ok, reason):
    if not ok:
        raise RuntimeError(reason)


def regular(path):
    # Every allowlisted relative component must be a real directory/file, never a link.
    need(not path.is_symlink() and stat.S_ISREG(path.stat().st_mode), "non_regular_file")


def body(path):
    regular(path)
    return path.read_bytes()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def verify_sources():
    manifest = json.loads(body(ROOT / MANIFEST))
    need(manifest.get("schema") == "generic-native-lines-public-manifest-v1", "manifest_schema")
    need(set(manifest["files"]) == set(SOURCES), "manifest_allowlist")
    pins = {}
    for name in (*SOURCES, MANIFEST):
        path = ROOT / name
        need(all(not p.is_symlink() for p in path.parents if p != ROOT.parent), "source_parent_symlink")
        data = body(path)
        need(len(data) <= 128 * 1024 and not PRIVATE_PATH.search(data), "source_size_or_private_path")
        if name != MANIFEST:
            need(manifest["files"][name]["sha256"] == digest(data), "source_hash_mismatch")
            need(manifest["files"][name]["bytes"] == len(data), "source_length_mismatch")
        pins[name] = {"sha256": digest(data), "bytes": len(data)}
    return pins


def source_receipt(path, pins):
    path.write_text("".join(f"{entry['sha256']}  {name}\n" for name, entry in sorted(pins.items())), encoding="ascii")


def bounded_command(args, log, env, timeout):
    with log.open("wb") as stream:
        try:
            result = subprocess.run(args, cwd=ROOT, env=env, stdout=stream,
                                    stderr=subprocess.STDOUT, timeout=timeout, check=False)
            return result.returncode, False
        except subprocess.TimeoutExpired:
            return 124, True
        except OSError:
            return 127, False


def tree_bytes(root):
    total = 0
    for base, dirs, names in os.walk(root, followlinks=False):
        need(all(not (Path(base) / d).is_symlink() for d in dirs), "build_symlink")
        for name in names:
            path = Path(base) / name
            regular(path)
            total += path.stat().st_size
            need(total < MAX_BUILD_BYTES, "build_disk_budget_exceeded")
    return total


def run(output):
    need(os.environ.get("VALIDATION_REVIEWED") == "1", "explicit_generic_review_required")
    # This is NOT authorization to publish or dispatch anything.
    out = Path(output).absolute()
    need(not out.exists() and not out.is_symlink(), "output_must_be_fresh")
    out.mkdir()
    build = out / ".build"
    build.mkdir()
    started = time.monotonic()
    receipt = {"schema": "generic-native-lines-exit-v1", "status": "UNQUALIFIED",
               "exit_code": 2, "phase": "incomplete", "timed_out": False,
               "native_or_implementation_qualification_inferred_from_host_tests": False}
    write_json(out / "exit.json", receipt)
    pins = None
    code = 2
    try:
        pins = verify_sources()
        source_receipt(out / "source.sha256", pins)
        write_json(out / "source-verified.json", {"before": True, "after": False, "unchanged": False})
        # Reject non-macOS before invoking host discovery or toolchain shims.
        # In particular, do not enter Windows WMI just to report an unsupported host.
        tools = {"schema": "generic-native-lines-toolchain-v1", "python": platform.python_version(),
                 "host_system": sys.platform,
                 "host_architecture": os.uname().machine if sys.platform == "darwin" else "not_collected"}
        write_json(out / "toolchain.json", tools)
        need(sys.platform == "darwin", "unsupported_host_no_native_metal")
        for name in ("rustc", "cargo"):
            p = subprocess.run([name, "--version"], capture_output=True, timeout=10, check=False)
            need(p.returncode == 0 and len(p.stdout) < 1024, "tool_version_unavailable")
            tools[name] = p.stdout.decode("utf-8").strip()
        write_json(out / "toolchain.json", tools)
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(build / "target")
        env["CARGO_BUILD_JOBS"] = "1"
        env["CARGO_INCREMENTAL"] = "0"
        env["RUST_TEST_THREADS"] = "1"
        # The toolchain is preinstalled. Never download/install Rust, crates, SDKs or drivers.
        receipt["phase"] = "host_tests"
        rc, timeout = bounded_command(["cargo", "test", "--release", "--offline", "--locked", "--jobs", "1", "--", "--test-threads=1"], build / "host-tests.log", env, 60)
        write_json(out / "host-tests.json", {"exit_code": rc, "timed_out": timeout,
                   "passed": rc == 0, "native_evidence": False})
        receipt["timed_out"] = timeout
        need(rc == 0, "host_tests_failed")
        receipt["phase"] = "native_build"
        rc, timeout = bounded_command(["cargo", "build", "--release", "--offline", "--locked", "--jobs", "1"], build / "build.log", env, 60)
        receipt["timed_out"] = timeout
        need(rc == 0, "native_build_failed")
        tools["native_executable_sha256"] = digest(body(build / "target/release/generic-native-lines"))
        tools["build_tree_bytes"] = tree_bytes(build)
        write_json(out / "toolchain.json", tools)
        receipt["phase"] = "native_measurement"
        rc, timeout = bounded_command([str(build / "target/release/generic-native-lines"),
                                      "--out", str(out / "results")], build / "native.log", env, 270)
        receipt["native_exit_code"] = rc
        receipt["timed_out"] = timeout
        need(verify_sources() == pins, "sources_changed_during_run")
        write_json(out / "source-verified.json", {"before": True, "after": True, "unchanged": True})
        need(rc == 0 and not timeout, "native_measurement_unqualified")
        summary = json.loads(body(out / "results/summary.json"))
        native = json.loads(body(out / "results/run.json"))
        need(summary["status"] == "QUALIFIED_NATIVE_MEASUREMENT" and summary["valid_measurements"] == EXPECTED_MEASUREMENTS
             and summary["expected_cases"] == EXPECTED_CASES
             and summary["expected_measurements"] == EXPECTED_MEASUREMENTS
             and summary["raw_readback_bytes"] == EXPECTED_CASES * 8 * 8704
             and summary["failed_checks"] == 0 and native["native_initialized"]
             and native["metadata"]["paravirtual_name_match"], "inconsistent_native_qualification")
        receipt.update(status="QUALIFIED_NATIVE_MEASUREMENT", exit_code=0, phase="complete")
        code = 0
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        # Exception messages can include private local paths: use only our fixed reason tokens.
        receipt["reason"] = str(error) if isinstance(error, RuntimeError) else "runner_or_io_failure"
    finally:
        if pins is not None:
            try:
                same = verify_sources() == pins
                write_json(out / "source-verified.json", {"before": True, "after": True, "unchanged": same})
                if not same:
                    code = 2
                    receipt.update(status="UNQUALIFIED", exit_code=2, reason="source_integrity_failed")
            except (OSError, ValueError, KeyError, RuntimeError):
                code = 2
                receipt.update(status="UNQUALIFIED", exit_code=2, reason="source_integrity_failed")
        receipt["elapsed_seconds"] = round(time.monotonic() - started, 3)
        write_json(out / "exit.json", receipt)
    print(receipt["status"] + ": " + receipt.get("reason", "bounded_native_measurements_complete"))
    return code


def export(output, destination):
    out, dest = Path(output).absolute(), Path(destination).absolute()
    need(out.is_dir() and not out.is_symlink() and not dest.exists() and not dest.is_symlink(), "export_directories")
    files, total = {}, 0
    for base, dirs, names in os.walk(out, followlinks=False):
        need(all(not (Path(base) / d).is_symlink() for d in dirs), "export_directory_symlink")
        dirs[:] = [d for d in dirs if d != ".build"]
        for name in names:
            path = Path(base) / name
            relative = path.relative_to(out)
            parts = relative.parts
            need((len(parts) == 1 and name in ROOT_RESULTS) or
                 (len(parts) == 2 and parts[0] == "results" and name in NATIVE_RESULTS), "artifact_not_allowlisted")
            data = body(path)
            need(len(data) < (MAX_RAW_BYTES if name == "readbacks.bin" else 12 * 1024**2), "artifact_file_bound")
            total += len(data)
            need(total < MAX_EXPORT_BYTES, "artifact_total_bound")
            if name != "readbacks.bin":
                need(not PRIVATE_PATH.search(data), "artifact_contains_private_path")
                if name.endswith(".json"):
                    json.loads(data)
                elif name.endswith(".jsonl"):
                    for row in data.splitlines():
                        json.loads(row)
            files[relative.as_posix()] = data
    need("exit.json" in files and 0 < len(files) <= 12, "artifact_count_or_exit_receipt")
    receipt = json.loads(files["exit.json"])
    if receipt["status"] == "QUALIFIED_NATIVE_MEASUREMENT":
        need(set(files) == ROOT_RESULTS | {"results/" + x for x in NATIVE_RESULTS}, "qualified_artifacts_incomplete")
        need(receipt["exit_code"] == 0, "qualified_exit_inconsistent")
    pins = {}
    dest.mkdir()
    for name, data in sorted(files.items()):
        target = dest / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        need(digest(body(target)) == digest(data), "export_copy_changed")
        pins[name] = {"bytes": len(data), "sha256": digest(data)}
    commit = os.environ.get("GITHUB_SHA", "")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    need(not commit or re.fullmatch(r"[0-9a-f]{40}", commit), "commit_metadata")
    need(not run_id or run_id.isdecimal(), "run_metadata")
    need(not attempt or attempt == "1", "automatic_retry_not_allowed")
    write_json(dest / "EXPORT.json", {"schema": "generic-native-export-v1", "files": pins,
        "bytes": total, "status": receipt["status"], "reviewed_commit": commit or None,
        "run_id": run_id or None, "run_attempt": attempt or None,
        "implementation_qualified": False, "line_coverage_rule": "UNKNOWN"})
    print("BOUNDED_GENERIC_EXPORT: " + str(len(files)) + " files")
    return 0


def bundle(destination):
    pins = verify_sources()
    dest = Path(destination)
    need(not dest.exists() and not dest.is_symlink(), "source_bundle_must_be_fresh")
    dest.mkdir()
    for name in sorted(pins):
        target = dest / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
        need(digest(body(target)) == pins[name]["sha256"], "source_bundle_copy_changed")
    print("GENERIC_SOURCE_BUNDLE: " + str(len(pins)) + " files; review is still required")
    return 0


def main():
    args = sys.argv[1:]
    if args == ["verify-sources"]:
        print("SOURCE_MANIFEST_VALID: " + str(len(verify_sources())) + " generic files")
        return 0
    if len(args) == 2 and args[0] == "run":
        return run(args[1])
    if len(args) == 3 and args[0] == "export":
        return export(args[1], args[2])
    if len(args) == 2 and args[0] == "source-bundle":
        return bundle(args[1])
    raise RuntimeError("usage_verify_sources_or_run_or_export_or_source_bundle")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print("UNQUALIFIED: " + (str(error) if isinstance(error, RuntimeError) else "review_io_or_schema_failure"), file=sys.stderr)
        sys.exit(2)

