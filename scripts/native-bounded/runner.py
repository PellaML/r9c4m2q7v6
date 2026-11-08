#!/usr/bin/env python3
"""Offline installed-toolchain runner; no installs, downloads or remote actions."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

import cases

SOURCE_FILES = (
    ".github/workflows/native-bounded.yml",
    "scripts/native-bounded/common.swift", "scripts/native-bounded/main.swift",
    "scripts/native-bounded/q1.swift", "scripts/native-bounded/q2.swift",
    "scripts/native-bounded/probe.metal", "scripts/native-bounded/cases.py",
    "scripts/native-bounded/runner.py", "scripts/native-bounded/run.sh",
)
NATIVE_LABELS = ("q1-shared", "q1-managed", "q1-private", "q2")
OUTPUT_FILES = frozenset((
    "metadata.json", "source-sha256.json", "commands.json", "run-status.json",
    "toolchain.txt", "build.txt", "self-test.txt", "validation.txt",
    "cases.json", "q1-patterns.bin", "q2-source.bin", "q2-seed.bin",
    "probe.air", "probe.metallib", "compile-pair.json", "validation.json", "q2-models.tsv",
) + tuple(name + suffix for name in NATIVE_LABELS for suffix in (".json", ".gpu.bin", ".txt")))
MAX_FILE, MAX_TOTAL, MAX_COUNT = 1_048_576, 8_388_608, 64
REFERENCE_BUILDS = {"macos-26": "25G332", "xcode-27": "26A434"}


def utc():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def write_json(path, obj):
    data = cases.encode_json(obj)
    cases.require(len(data) <= MAX_FILE, "json-budget")
    path.write_bytes(data)


def sha_file(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def retained_files(out):
    cases.require(out.is_dir() and not out.is_symlink(), "output-root-kind")
    files = []
    for p in sorted(out.iterdir()):
        cases.require(not p.is_symlink(), "output-symlink")
        if p.name == ".build":
            cases.require(p.is_dir(), "build-directory-kind")
            continue  # Never retained or uploaded.
        cases.require(p.is_file(), "unexpected-output-directory")
        if p.name in ("pins.json", "pins.sha256"):
            continue
        cases.require(p.name in OUTPUT_FILES, "unexpected-output-file")
        files.append(p)
    cases.require(1 <= len(files) <= MAX_COUNT, "file-count-budget")
    cases.require(all(p.stat().st_size <= MAX_FILE for p in files), "file-byte-budget")
    cases.require(sum(p.stat().st_size for p in files) <= MAX_TOTAL - 65536, "total-byte-budget")
    return files


def seal(out):
    files = retained_files(out)
    pins = dict(schema=1, files=[dict(name=p.name, bytes=p.stat().st_size, sha256=sha_file(p)) for p in files],
                file_count=len(files), total_bytes=sum(p.stat().st_size for p in files),
                budgets=dict(per_file=MAX_FILE, total=MAX_TOTAL, count=MAX_COUNT),
                exclusion=".build contains local executables/caches only and is never an artifact",
                manifest_self_pin="pins.sha256 hashes pins.json; pins.json lists every other retained file")
    write_json(out / "pins.json", pins)
    (out / "pins.sha256").write_text(sha_file(out / "pins.json") + "  pins.json\n", encoding="ascii")
    verify(out)


def verify(out):
    out = Path(out)
    files = retained_files(out)
    for name in ("pins.json", "pins.sha256"):
        p = out / name
        cases.require(p.is_file() and not p.is_symlink() and p.stat().st_size <= MAX_FILE, "manifest-kind-budget")
    pin_line = (out / "pins.sha256").read_text(encoding="ascii")
    cases.require(pin_line == sha_file(out / "pins.json") + "  pins.json\n", "manifest-pin")
    pins = json.loads((out / "pins.json").read_text())
    expected = [dict(name=p.name, bytes=p.stat().st_size, sha256=sha_file(p)) for p in files]
    cases.require(pins.get("schema") == 1 and pins.get("files") == expected, "retained-file-pin-or-inventory")
    cases.require(pins.get("file_count") == len(files) and pins.get("total_bytes") == sum(p.stat().st_size for p in files), "manifest-counts")
    cases.require(sum(p.stat().st_size for p in files) + (out / "pins.json").stat().st_size + len(pin_line) <= MAX_TOTAL, "sealed-total-budget")
    return dict(status="VERIFIED_BOUNDED_OUTPUT_PINS", files=len(files) + 2)


def run():
    here = Path(__file__).resolve().parent
    root = here.parent.parent
    out = Path.cwd() / "out-native-bounded"
    # Refuse reuse, including dangling symlinks. No deletion/clean command.
    cases.require(not out.exists() and not out.is_symlink(), "fresh-output-directory-required")
    out.mkdir(); (out / ".build").mkdir(); (out / ".build" / "tmp").mkdir()
    commands = []
    metadata = dict(schema=1, started_utc=utc(), host_platform=platform.system(),
                    python_version=sys.version, python_executable_name=Path(sys.executable).name,
                    python_executable_sha256=sha_file(sys.executable),
                    runner_label=os.environ.get("RUNNER_LABEL", "local"),
                    expected_os_build=os.environ.get("EXPECTED_OS_BUILD", "missing"),
                    native_reference_builds=REFERENCE_BUILDS, other_build_equivalence_claimed=False,
                    native_execution_attempted=False, environment_dumped=False)
    status = dict(schema=1, status="NOT_QUALIFIED", run_exit=3, native_exits={},
                  cpu_result_fallback=False, resource_substitution=False,
                  runner_publication_performed=False, runner_remote_operations_performed=False)
    env = os.environ.copy()
    env.update(PYTHONDONTWRITEBYTECODE="1", TMPDIR=str(out / ".build" / "tmp"))

    def public_text(text):
        return text.replace(str(root), "<PUBLIC_SOURCE_ROOT>").replace(str(Path.home()), "<HOME>")

    def invoke(label, argv, logfile, timeout=120, required=True):
        begin = utc(); start = time.monotonic()
        try:
            r = subprocess.run(argv, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout, check=False)
            rc, output = r.returncode, r.stdout
        except subprocess.TimeoutExpired as e:
            rc, output = 124, (e.stdout or b"") + b"\n[command timeout; NOT QUALIFIED]\n"
        except OSError:
            rc, output = 127, b"[installed command unavailable; NOT QUALIFIED]\n"
        truncated = len(output) > 65536
        full_hash = cases.digest(output)
        retained = output[:65536] + (b"\n[bounded log; full-output hash recorded; NOT QUALIFIED]\n" if truncated else b"")
        # Public compiler diagnostics may mention the checkout path; do not retain
        # that location. Exact argument vectors use relative paths from repo root.
        if label in NATIVE_LABELS:
            # API assertions/crash diagnostics may contain object addresses or
            # driver internals. Never retain those arbitrary native strings.
            retained = (f"native_process_exit={rc}\nraw_diagnostics_withheld=true\n"
                        f"output_bytes={len(output)}\noutput_sha256={full_hash}\n").encode()
        else:
            retained = public_text(retained.decode("utf-8", errors="replace")).encode()
        log = out / logfile
        with log.open("ab") as f:
            f.write(("\n[" + label + "]\n").encode() + retained)
        recorded_argv = ["<PYTHON_EXECUTABLE_PINNED_IN_METADATA>" if a == sys.executable else public_text(a) for a in argv]
        commands.append(dict(label=label, argv=recorded_argv, cwd="public-repository-root", started_utc=begin,
                             completed_utc=utc(), elapsed_seconds=round(time.monotonic()-start, 6),
                             exit=rc, output_sha256_before_path_redaction=full_hash,
                             output_bytes=len(output), output_truncated=truncated, log=logfile))
        write_json(out / "commands.json", commands)
        cases.require(log.stat().st_size <= MAX_FILE, "command-log-budget")
        if required:
            cases.require(rc == 0 and not truncated, "command-failed:" + label)
        return rc if not truncated else 3, output.decode("utf-8", errors="replace").strip()

    try:
        write_json(out / "metadata.json", metadata)
        cases.require(platform.system() == "Darwin", "Metal-unavailable-non-Darwin-no-substitute")
        cases.require(Path.cwd().resolve() == root, "run-from-public-repository-root")
        source_pins = []
        for name in SOURCE_FILES:
            p = root / name
            cases.require(p.is_file() and not p.is_symlink(), "source-allowlist-file-missing")
            source_pins.append(dict(name=name, bytes=p.stat().st_size, sha256=sha_file(p)))
        write_json(out / "source-sha256.json", source_pins)
        for label, argv in (
            ("uname", ["uname", "-s"]), ("architecture", ["uname", "-m"]),
            ("os", ["sw_vers"]), ("os-build", ["sw_vers", "-buildVersion"]),
            ("xcode", ["xcodebuild", "-version"]), ("developer-dir", ["xcode-select", "-p"]),
            ("sdk-path", ["xcrun", "--sdk", "macosx", "--show-sdk-path"]),
            ("sdk-version", ["xcrun", "--sdk", "macosx", "--show-sdk-version"]),
            ("sdk-build", ["xcrun", "--sdk", "macosx", "--show-sdk-build-version"]),
            ("metal-version", ["xcrun", "--sdk", "macosx", "metal", "--version"]),
            ("swift-version", ["xcrun", "--sdk", "macosx", "swiftc", "--version"]),
            ("swift-target", ["xcrun", "--sdk", "macosx", "swiftc", "-print-target-info"]),
        ):
            _, output = invoke(label, argv, "toolchain.txt", timeout=45)
            metadata[label] = public_text(output)
            if label == "os-build":
                metadata["actual_os_build"] = output
        metadata["actual_os_build"] = metadata["os-build"]
        env["NATIVE_OS_BUILD"] = metadata["actual_os_build"]
        write_json(out / "metadata.json", metadata)
        expected = metadata["expected_os_build"]
        cases.require(expected in REFERENCE_BUILDS.values() and metadata["actual_os_build"] == expected, "OS-build-drift-or-missing-explicit-reference")
        label = metadata["runner_label"]
        cases.require(label == "local" or REFERENCE_BUILDS.get(label) == expected, "runner-label-build-mismatch")
        cases.require(metadata["architecture"] in ("arm64", "x86_64"), "unsupported-host-architecture")
        binaries = {}
        for tool in ("metal", "metallib", "swiftc"):
            _, location = invoke("resolve-" + tool, ["xcrun", "--sdk", "macosx", "-f", tool], "toolchain.txt", timeout=30)
            p = Path(location)
            cases.require(p.is_file(), "compiler-executable-missing")
            binaries[tool] = dict(path=public_text(location), sha256=sha_file(p), bytes=p.stat().st_size)
        metadata["tool_binaries"] = binaries
        metadata["metal_environment"] = {k: env.get(k, "unset") for k in ("MTL_DEBUG_LAYER", "MTL_SHADER_VALIDATION")}
        metadata["compiler_input_paths"] = "relative to checked-out public repository root"
        write_json(out / "metadata.json", metadata)
        invoke("self-tests", [sys.executable, "-B", "scripts/native-bounded/cases.py", "self-test"], "self-test.txt")
        generated = cases.generate(out)
        status["generated_input_pins"] = generated
        metal = ["xcrun", "--sdk", "macosx", "metal", "-std=metal3.1", "-O2", "-g0", "-fno-fast-math",
                 "-c", "scripts/native-bounded/probe.metal", "-o", "out-native-bounded/probe.air"]
        invoke("metal-compile", metal, "build.txt")
        air_sha = sha_file(out / "probe.air")
        link = ["xcrun", "--sdk", "macosx", "metallib", "out-native-bounded/probe.air", "-o", "out-native-bounded/probe.metallib"]
        invoke("metallib-link", link, "build.txt")
        cases.require(air_sha == sha_file(out / "probe.air"), "AIR-changed-across-link")
        pair = dict(source_sha256=sha_file(here / "probe.metal"), air_sha256=air_sha,
                    metallib_sha256=sha_file(out / "probe.metallib"), compile_exit=0, link_exit=0,
                    link_input="probe.air", runtime_library="probe.metallib", language="metal3.1",
                    compile_argv=metal, link_argv=link, source_compile_fallback=False)
        write_json(out / "compile-pair.json", pair)
        swift = ["xcrun", "--sdk", "macosx", "swiftc", "-swift-version", "5", "-O", "-target",
                 metadata["architecture"] + "-apple-macos14.0", "-module-cache-path", "out-native-bounded/.build/module-cache"]
        swift += ["scripts/native-bounded/" + name for name in ("common.swift", "q1.swift", "q2.swift", "main.swift")]
        swift += ["-o", "out-native-bounded/.build/probe"]
        invoke("swift-build", swift, "build.txt")
        metadata["swift_compile_argv"] = swift
        metadata["native_execution_attempted"] = True
        write_json(out / "metadata.json", metadata)
        for probe in NATIVE_LABELS:
            # Separate processes preserve other mode observations on native asserts,
            # nil returns, API errors or timeouts. None is called a supported pass.
            rc, _ = invoke(probe, ["out-native-bounded/.build/probe", probe, "scripts/native-bounded", "out-native-bounded"],
                           probe + ".txt", timeout=180, required=False)
            status["native_exits"][probe] = rc
            if not (out / (probe + ".json")).exists():
                write_json(out / (probe + ".json"), dict(schema=1, probe=probe, status="FAILED_NOT_PASS",
                    reason="native-process-no-report", exit_code=rc, cpu_result_fallback=False))
        validation_rc, _ = invoke("independent-validation", [sys.executable, "-B", "scripts/native-bounded/cases.py", "validate", "out-native-bounded"],
                                  "validation.txt", required=False)
        status["validation_exit"] = validation_rc
        cases.require(validation_rc == 0 and all(x == 0 for x in status["native_exits"].values()), "native-lane-or-validation-not-qualified")
        status.update(status="BOUNDED_NATIVE_OBSERVATIONS_VALIDATED", run_exit=0)
    except (cases.Refusal, OSError, ValueError, KeyError) as e:
        status["failure"] = str(e) if isinstance(e, cases.Refusal) else type(e).__name__
    finally:
        status["finished_utc"] = utc()
        write_json(out / "metadata.json", metadata)
        write_json(out / "commands.json", commands)
        write_json(out / "run-status.json", status)
        try:
            seal(out)
        except (cases.Refusal, OSError, ValueError) as e:
            status.update(run_exit=3, status="PACKAGING_FAILED", packaging_failure=type(e).__name__)
            write_json(out / "run-status.json", status)
        print(cases.encode_json(status).decode())
    return status["run_exit"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("run", "verify"))
    parser.add_argument("out", nargs="?", type=Path)
    args = parser.parse_args()
    if args.command == "verify":
        print(cases.encode_json(verify(args.out or Path("out-native-bounded"))).decode())
        return 0
    cases.require(args.out is None, "run-takes-no-output-argument")
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
