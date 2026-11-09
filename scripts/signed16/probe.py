#!/usr/bin/env python3
"""Inputs, exact-AIR call proof, raw-control validation and installed-tool runner.
No conversion oracle generates GPU outputs. Out-of-domain values are CPU-refused.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import struct
import subprocess
import sys
import unittest

FORWARD = "air.convert.s.v2i16.f.v2f32"
INVERSE = "air.convert.f.v2f32.s.v2i16"
SOURCES = (".github/workflows/signed16.yml", "scripts/signed16/probe.metal",
           "scripts/signed16/probe.swift", "scripts/signed16/probe.py")
MODES = {"precise": "-fno-fast-math", "fast": "-ffast-math"}
BUILDS = {"macos-26": "25G332", "xcode-27": "26A434"}
STEPS = (("forward", "forward_s16", 20, "i16", "cpu:float-input.bin"),
         ("chain", "inverse_s16", 20, "f32", "gpu:forward"),
         ("standalone", "inverse_s16", 3, "f32", "cpu:inverse-input.bin"),
         ("swap", "swap_s16", 20, "i16", "cpu:float-input.bin"),
         ("swap-chain", "inverse_s16", 20, "f32", "gpu:swap"),
         ("nearest", "nearest_s16", 4, "i16", "cpu:float-input.bin:first4-safe"),
         ("nearest-chain", "inverse_s16", 4, "f32", "gpu:nearest"))
OUTPUTS = frozenset(("metadata.json", "commands.json", "source-sha256.json", "run-status.json",
    "inputs.json", "float-input.bin", "inverse-input.bin", "domain-check.json", "self-test.txt",
    "toolchain.txt", "build.txt", "validation.json") + tuple(m+s for m in MODES for s in
    (".air", ".ll", ".ir-proof.json", ".compile.json", ".metallib", ".gpu.json", ".gpu.bin", ".native.txt", ".observed.tsv")))


def need(ok, reason):
    if not ok: raise ValueError(reason)


def sha(d): return hashlib.sha256(d).hexdigest()
def encoded(obj): return (json.dumps(obj, sort_keys=True, indent=2, allow_nan=False)+"\n").encode()
def write_json(path, obj): path.write_bytes(encoded(obj))
def utc(): return datetime.now(timezone.utc).isoformat(timespec="seconds")
def f32(bits): return struct.unpack("<f", struct.pack("<I", bits))[0]


def admitted(bits):
    x = f32(bits)
    return math.isfinite(x) and -32769 < x < 32768


def pairs():
    # The first four pairs are the ONLY inputs to the nearest-rounding negative.
    return [(0x3f400000,0xbf400000), (0x3fc00000,0xbfc00000),
        (0x40f00000,0xc0f00000), (0xc0100000,0x40780000),
        (0x00000000,0x80000000), (0x80000000,0x00000000),
        (0x00000001,0x80000001), (0x80000001,0x00000001),
        (0x007fffff,0x807fffff), (0x807fffff,0x007fffff),
        (0x3f000000,0xbf000000), (0xbf000000,0x3f000000),
        (0x3f800000,0xbf800000), (0xbf800000,0x3f800000),
        (0x3f7fffff,0xbf7fffff), (0x3f800001,0xbf800001),
        (0x46fffe00,0xc7000000), (0x46ffff00,0xc7000080),
        (0x46ffffff,0xc70000ff), (0xc70000ff,0x46ffffff)]


def independent_i16(): return [(0x0000,0x0001), (0x7fff,0x8000), (0xffff,0x0000)]


def domain_gate(rows):
    need(len(rows) == 20 and all(len(r)==2 for r in rows), "input_pair_bounds")
    need(all(admitted(x) for row in rows for x in row), "CPU_REFUSED_outside_finite_open_RTZ_domain")
    need(all(abs(f32(x)) <= 16 for row in rows[:4] for x in row), "CPU_REFUSED_unsafe_rounding_negative")


def inputs():
    rows = pairs(); domain_gate(rows)
    plan = dict(schema=1, count=20, inverseCount=3, nearestCount=4, guardBytes=64,
        forward_intrinsic=FORWARD, inverse_intrinsic=INVERSE,
        domain="finite binary32, strict open interval (-32769,32768); no native out-of-domain dispatch",
        rows=[dict(id=i, source_f32_hex=[f"{x:08x}" for x in row]) for i,row in enumerate(rows)],
        independent_i16_hex=[[f"{x:04x}" for x in row] for row in independent_i16()],
        outputs="native i16 words and inverse f32 bits only; no CPU-produced conversion expectations")
    return {"inputs.json": encoded(plan), "float-input.bin": b"".join(struct.pack("<II", *r) for r in rows),
            "inverse-input.bin": b"".join(struct.pack("<HH", *r) for r in independent_i16())}


SSA = r"%[-A-Za-z0-9_.$]+"


def prove_call(ir, kernel, intrinsic, result_type, argument_type, lane_bits):
    text = re.sub(r";[^\n]*", "", ir)
    match = re.search(r'(?ms)^(define[^\n]*@"?'+re.escape(kernel)+r'"?\([^{}]*\)[^\n]*\{)(.*?)^\}', text)
    need(match is not None, "missing_kernel_definition:"+kernel)
    header, body = match.groups()
    arguments = re.findall(SSA, header)
    need(len(arguments) >= 2, "kernel_arguments")
    input_arg, output_arg = arguments[:2]
    need((INVERSE if intrinsic == FORWARD else FORWARD) not in body, "paired_conversions_in_one_kernel")
    need(not re.search(r"\b(fptosi|fptoui|sitofp|uitofp|fpext|fptrunc)\b", body), "alternate_float_conversion_route")
    definitions = {}
    for line in body.splitlines():
        m = re.match(r"\s*("+SSA+r")\s*=\s*(.*)", line)
        if m: definitions[m[1]] = m[2]

    def ancestors(value, seen=None):
        seen = set() if seen is None else seen
        if value in seen: return seen
        seen.add(value)
        for operand in re.findall(SSA, definitions.get(value, "")):
            ancestors(operand, seen)
        return seen

    matches = []
    for value, rhs in definitions.items():
        needle = "@"+intrinsic+"("
        if needle not in rhs or not re.search(r"\bcall\b", rhs): continue
        before, after = rhs.split(needle, 1)
        need(before.rstrip().endswith(result_type), "wrong_result_type")
        arg = re.match(re.escape(argument_type)+r"\s+(?:[A-Za-z_]+\s+)*("+SSA+r")", after)
        need(arg is not None, "wrong_argument_type_or_constant_argument")
        ancestry = ancestors(arg[1])
        need(input_arg in ancestry and any(definitions.get(a, "").startswith("load ") for a in ancestry), "conversion_not_from_input_load")
        live = {value: {0,1}}
        for _ in range(len(definitions)+1):
            before_count = len(live)
            for alias, expr in definitions.items():
                deps = re.findall(SSA, expr)
                if alias in live or not deps or deps[0] not in live: continue
                if expr.startswith("bitcast "):
                    live[alias] = set(live[deps[0]])
                elif expr.startswith("extractelement "):
                    lane = re.search(r",\s*i(?:32|64)\s+([01])(?:\s|$)", expr)
                    if lane: live[alias] = {int(lane[1])}
            if len(live) == before_count: break
        stored, stores = set(), []
        for line in body.splitlines():
            st = re.match(r"\s*store\s+(<\d+ x \w+>|i\d+|float)\s+("+SSA+r"),\s*([^,]+)", line)
            if not st or st[2] not in live: continue
            pointers = re.findall(SSA, st[3])
            if not pointers or output_arg not in ancestors(pointers[-1]): continue
            ty = st[1]
            width = {"<2 x i16>":32, "i32":32, "<2 x float>":64, "<2 x i32>":64,
                     "i64":64, "i16":16, "float":32}.get(ty)
            if width == lane_bits*len(live[st[2]]):
                stored |= live[st[2]]; stores.append(line.strip())
        need(stored == {0,1}, "conversion_result_not_stored_to_actual_output_both_lanes")
        matches.append(dict(kernel=kernel, intrinsic=intrinsic, call=value+" = "+rhs,
                            input_load_proven=True, output_stores=stores, lanes_stored=[0,1]))
    need(len(matches) == 1, "missing_or_ambiguous_live_exact_call:"+intrinsic)
    return matches[0]


def inspect_ir(data):
    ir = data.decode("utf-8")
    calls = [prove_call(ir,"forward_s16",FORWARD,"<2 x i16>","<2 x float>",16),
             prove_call(ir,"inverse_s16",INVERSE,"<2 x float>","<2 x i16>",32)]
    return dict(schema=1, qualified=True, ir_sha256=sha(data), calls=calls,
        scope="Typed live vector calls in separate kernel bodies, input-load to output-store; exact AIR/library binding required separately; not GPU ISA proof")


def guard(data, count, stride):
    need(len(data)==128+count*stride and data[:64]==b"\xcd"*64 and data[-64:]==b"\xcd"*64, "raw_length_or_guard")


def payload(data, count, kind):
    stride = 4 if kind=="i16" else 8
    guard(data,count,stride)
    return [struct.unpack_from("<HH" if kind=="i16" else "<II",data,64+i*stride) for i in range(count)]


def validate_mode(out, mode):
    report = json.loads((out/(mode+".gpu.json")).read_text())
    need(report["status"]=="CAPTURED_PENDING_RAW_CONTROL_VALIDATION" and report["exit_code"]==0 and report["native_started"], "native_not_complete")
    need(not report["cpu_result_fallback"] and not report["source_compile_fallback"], "no_fallback")
    need(report["input_sha256"]=={n:sha(d) for n,d in inputs().items()}, "native_input_pins")
    pair=json.loads((out/(mode+".compile.json")).read_text())
    need(report["compile_pair"]==pair and report["loaded_library_sha256"]==sha((out/(mode+".metallib")).read_bytes()), "loaded_library_pin")
    need(pair["air_sha256"]==sha((out/(mode+".air")).read_bytes()) and pair["ir_sha256"]==sha((out/(mode+".ll")).read_bytes()) and
         pair["proof_sha256"]==sha((out/(mode+".ir-proof.json")).read_bytes()), "independent_AIR_IR_proof_pins")
    need(inspect_ir((out/(mode+".ll")).read_bytes())==json.loads((out/(mode+".ir-proof.json")).read_text()), "independent_live_call_recheck")
    need(report["same_GPU_intermediate_buffer_consumed"] and report["intermediate_saved_before_inverse"], "independent_intermediate_capture")
    metadata=json.loads((out/"metadata.json").read_text())
    need(report["os_build_from_runner"]==metadata["actual_os_build"] and report.get("device_architecture") and report.get("device"), "native_device_build")
    need(len(report["commands"])==7 and len(report["records"])==14, "native_coverage")
    blob=(out/(mode+".gpu.bin")).read_bytes()
    need(len(blob)==report["raw_bytes"]==3800 and sha(blob)==report["raw_sha256"], "raw_file_pin")
    spans={}; offset=0
    for r in report["records"]:
        need(r["offset"]==offset, "span_sequence")
        d=blob[offset:offset+r["length"]]; offset+=len(d)
        need(len(d)==r["length"] and sha(d)==r["sha256"] and (r["name"],r["kind"]) not in spans, "span_pin_or_duplicate")
        spans[r["name"],r["kind"]]=(r,d)
    need(offset==len(blob), "raw_trailing_bytes")
    outputs={}; echoes={}
    for tag,(name,kernel,count,kind,label) in enumerate(STEPS,1):
        command=report["commands"][tag-1]
        need(all(command[k]==v for k,v in dict(name=name,kernel=kernel,count=count,input=label,tag=tag,storage=0,status=4,completed=True,error_present=False).items()), "command_contract")
        need(command["input_sha256_before"]==command["input_sha256_after"], "input_mutation")
        r,data=spans[name,kind]; tr,trace=spans[name,"trace"]
        for rec in (r,tr):
            need(all(rec[k]==v for k,v in dict(name=name,kernel=kernel,count=count,input=label,tag=tag).items()), "record_identity")
        outputs[name]=payload(data,count,kind); guard(trace,count,16)
        if label.startswith("gpu:"):
            producer=label[4:]; expected_echo=outputs[producer]
            need(command["input_sha256_before"]==sha(spans[producer,"i16"][1]), "inverse_consumed_recorded_GPU_buffer")
        elif name=="standalone": expected_echo=independent_i16()
        else: expected_echo=[tuple(reversed(x)) for x in pairs()] if name=="swap" else pairs()[:count]
        echoes[name]=[]
        for i in range(count):
            magic,index,a,b=struct.unpack_from("<4I",trace,64+i*16)
            need(magic==(0x53160000^tag) and index==i and (a,b)==expected_echo[i], "trace_marker_or_input_echo")
            echoes[name].append((a,b))
    need(outputs["swap"]==[tuple(reversed(x)) for x in outputs["forward"]] and outputs["swap"]!=outputs["forward"], "genuine_lane_negative_i16")
    need(outputs["swap-chain"]==[tuple(reversed(x)) for x in outputs["chain"]] and outputs["swap-chain"]!=outputs["chain"], "genuine_lane_negative_inverse")
    need(outputs["nearest"]!=outputs["forward"][:4] and outputs["nearest-chain"]!=outputs["chain"][:4], "genuine_rounding_negative_not_distinct")
    # This is a second GPU inverse observation, not a CPU numerical oracle.
    reference={}; hits=set()
    for src, result in zip(independent_i16(),outputs["standalone"]):
        for word,bits in zip(src,result):
            need(word not in reference or reference[word]==bits,"standalone_repeat_disagrees")
            reference[word]=bits
    for src,result in zip(outputs["forward"],outputs["chain"]):
        for word,bits in zip(src,result):
            if word in reference:
                need(reference[word]==bits,"chain_vs_independent_native_inverse")
                hits.add(word)
    need(hits==set(reference),"standalone_inverse_cross_reference_coverage")
    lines=["mode\tgroup\trow\tsource_lane0_bits\tsource_lane1_bits\tGPU_i16_result_or_input_echo_lane0\tGPU_i16_result_or_input_echo_lane1\tGPU_inverse_f32_lane0\tGPU_inverse_f32_lane1"]
    for group,source_rows,int_rows,float_rows in (
        ("forward-chain",pairs(),outputs["forward"],outputs["chain"]),
        ("swapped",[tuple(reversed(x)) for x in pairs()],outputs["swap"],outputs["swap-chain"]),
        ("nearest-negative",pairs()[:4],outputs["nearest"],outputs["nearest-chain"]),
        ("standalone-inverse",independent_i16(),echoes["standalone"],outputs["standalone"])):
        for i,(src,integers,floats) in enumerate(zip(source_rows,int_rows,float_rows)):
            lines.append("\t".join([mode,group,str(i),*[f"{x:08x}" for x in src],*[f"{x:04x}" for x in integers],*[f"{x:08x}" for x in floats]]))
    (out/(mode+".observed.tsv")).write_text("\n".join(lines)+"\n",encoding="ascii")
    return dict(status="NATIVE_RAW_CONTROLS_PASS_NUMERIC_REVIEW_PENDING",commands=7,records=14,raw_bytes=len(blob),
        lane_negative_different_pairs=sum(a!=b for a,b in zip(outputs["swap"],outputs["forward"])),
        rounding_negative_different_pairs=sum(a!=b for a,b in zip(outputs["nearest"],outputs["forward"][:4])),
        independent_inverse_words=[f"{x:04x}" for x in sorted(hits)],
        CPU_numeric_expected_outputs_generated=False,device=report["device"],os=report["os"])


def retained(out):
    need(out.is_dir() and not out.is_symlink(),"output_directory")
    files=[]
    for p in sorted(out.iterdir()):
        need(not p.is_symlink(),"output_symlink")
        if p.name==".build": need(p.is_dir(),"build_kind"); continue
        if p.name in ("pins.json","pins.sha256"): continue
        need(p.is_file() and p.name in OUTPUTS and p.stat().st_size<=1048576,"output_allowlist_or_size")
        files.append(p)
    need(1<=len(files)<=64 and sum(p.stat().st_size for p in files)<8*1024*1024-65536,"output_budget")
    return files


def seal(out):
    files=retained(out)
    write_json(out/"pins.json",dict(schema=1,files=[dict(name=p.name,bytes=p.stat().st_size,sha256=sha(p.read_bytes())) for p in files],
        build_directory_retained=False,per_file_limit=1048576,total_limit=8388608,file_count_limit=64))
    (out/"pins.sha256").write_text(sha((out/"pins.json").read_bytes())+"  pins.json\n",encoding="ascii")
    verify(out)


def verify(out):
    files=retained(out)
    for name in ("pins.json","pins.sha256"): need((out/name).is_file() and not (out/name).is_symlink(),"manifest_kind")
    need((out/"pins.sha256").read_text()==sha((out/"pins.json").read_bytes())+"  pins.json\n","manifest_pin")
    pins=json.loads((out/"pins.json").read_text())
    need(pins["files"]==[dict(name=p.name,bytes=p.stat().st_size,sha256=sha(p.read_bytes())) for p in files],"retained_pins")
    return dict(status="PINNED_OUTPUT_VERIFIED",files=len(files)+2)


class Tests(unittest.TestCase):
    def test_domain_and_boundary_inputs(self):
        domain_gate(pairs())
        for bits in (0,0x80000000,1,0x80000001,0x007fffff,0x807fffff,0xc70000ff,0x46ffffff): self.assertTrue(admitted(bits))
        for bits in (0xc7000100,0xc7000101,0x47000000,0x47000001,0x7f800000,0xff800000,0x7fc12345,0xff800001): self.assertFalse(admitted(bits))
        bad=pairs();bad[0]=(0x47000000,0)
        with self.assertRaisesRegex(ValueError,"CPU_REFUSED"): domain_gate(bad)
        bad=pairs();bad[0]=(0x46ffffff,0)
        with self.assertRaisesRegex(ValueError,"unsafe_rounding"): domain_gate(bad)

    def test_inputs_only(self):
        self.assertEqual(inputs(),inputs())
        self.assertEqual(set(inputs()),{"inputs.json","float-input.bin","inverse-input.bin"})
        self.assertEqual({x for row in independent_i16() for x in row},{0,1,0x7fff,0x8000,0xffff})
        self.assertEqual(pairs()[18],tuple(reversed(pairs()[19])))

    @staticmethod
    def fixture():
        # Parser fixture only, not a compiled artifact or GPU numerical output.
        return (f"define void @forward_s16(ptr %input, ptr %out) {{\n %x = load <2 x float>, ptr %input\n %y = call <2 x i16> @{FORWARD}(<2 x float> %x)\n store <2 x i16> %y, ptr %out\n ret void\n}}\n"
              +f"define void @inverse_s16(ptr %input, ptr %out) {{\n %x = load <2 x i16>, ptr %input\n %y = call <2 x float> @{INVERSE}(<2 x i16> %x)\n store <2 x float> %y, ptr %out\n ret void\n}}\n")

    def test_live_typed_separate_calls(self):
        self.assertTrue(inspect_ir(self.fixture().encode())["qualified"])
        packed=self.fixture().replace("store <2 x i16> %y, ptr %out","%packed = bitcast <2 x i16> %y to i32\n store i32 %packed, ptr %out")
        self.assertTrue(inspect_ir(packed.encode())["qualified"])

    def test_missing_dead_wrong_width_and_wrong_destination_refuse(self):
        for ir in ("declare <2 x i16> @"+FORWARD+"(<2 x float>)", self.fixture().replace("store <2 x i16> %y, ptr %out","store i32 0, ptr %out"),
                   self.fixture().replace(FORWARD,"air.convert.s.v2i32.f.v2f32"), self.fixture().replace("store <2 x i16> %y, ptr %out","store <2 x i16> %y, ptr %input"),
                   self.fixture().replace(" %x = load <2 x float>, ptr %input"," %x = load <2 x float>, ptr %out")):
            with self.assertRaises(ValueError): inspect_ir(ir.encode())

    def test_pair_in_one_body_refused(self):
        ir=self.fixture().replace(" store <2 x i16> %y",f" %z = call <2 x float> @{INVERSE}(<2 x i16> %y)\n store <2 x i16> %y",1)
        with self.assertRaisesRegex(ValueError,"paired_conversions"): inspect_ir(ir.encode())


def run():
    here=Path(__file__).resolve().parent; root=here.parent.parent; out=Path.cwd()/"out-signed16"
    need(not out.exists() and not out.is_symlink(),"fresh_output_required")
    out.mkdir();(out/".build").mkdir();(out/".build/tmp").mkdir()
    metadata=dict(schema=1,platform=platform.system(),python_version=sys.version,expected_os_build=os.environ.get("EXPECTED_OS_BUILD","missing"),
                  runner_label=os.environ.get("RUNNER_LABEL","local"),native_started=False)
    status=dict(status="NOT_QUALIFIED",exit=3,native_modes={},cpu_fallback=False,remote_operations_performed=False)
    commands=[]; env=os.environ.copy();env.update(PYTHONDONTWRITEBYTECODE="1",TMPDIR=str(out/".build/tmp"))
    def redact(x): return x.replace(str(root),"<PUBLIC_ROOT>").replace(str(Path.home()),"<HOME>")
    def invoke(label,argv,log="build.txt",required=True):
        try:
            p=subprocess.run(argv,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120,check=False)
            rc,data=p.returncode,p.stdout
        except subprocess.TimeoutExpired as e: rc,data=124,(e.stdout or b"")
        except OSError: rc,data=127,b"installed tool unavailable\n"
        truncated=len(data)>65536
        safe=(f"native_exit={rc}\nraw_native_diagnostics_withheld=true\nbytes={len(data)}\nsha256={sha(data)}\n" if label.startswith("native-")
              else redact(data[:65536].decode(errors="replace")))
        with (out/log).open("ab") as f: f.write(("\n["+label+"]\n"+safe).encode())
        commands.append(dict(label=label,argv=["<PYTHON>" if x==sys.executable else redact(x) for x in argv],exit=rc,utc=utc(),output_bytes=len(data),output_sha256=sha(data),truncated=truncated,log=log))
        write_json(out/"commands.json",commands)
        if required: need(rc==0 and not truncated,"command_failed:"+label)
        return (3 if truncated else rc),data.decode(errors="replace").strip()
    try:
        generated=inputs()
        for name,data in generated.items(): (out/name).write_bytes(data)
        refused=[0xc7000100,0xc7000101,0x47000000,0x47000001,0x7f800000,0xff800000,0x7fc12345,0xff800001]
        need(all(not admitted(x) for x in refused),"domain_guard_selfcheck")
        write_json(out/"domain-check.json",dict(status="CPU_DOMAIN_GUARD_PASS",all_40_forward_lanes_admitted=True,
            CPU_only_refused_bits=[f"{x:08x}" for x in refused],refused_values_dispatched=False,nearest_negative_safe_prefix_pairs=4))
        write_json(out/"metadata.json",metadata)
        need(platform.system()=="Darwin","non_Darwin_no_native_fallback")
        need(Path.cwd().resolve()==root,"run_from_public_repository_root")
        write_json(out/"source-sha256.json",[dict(name=n,bytes=len((root/n).read_bytes()),sha256=sha((root/n).read_bytes())) for n in SOURCES])
        for name,argv in (("os",["sw_vers"]),("os-build",["sw_vers","-buildVersion"]),("architecture",["uname","-m"]),
            ("xcode",["xcodebuild","-version"]),("developer-dir",["xcode-select","-p"]),
            ("sdk-version",["xcrun","--sdk","macosx","--show-sdk-version"]),("sdk-build",["xcrun","--sdk","macosx","--show-sdk-build-version"]),
            ("sdk-path",["xcrun","--sdk","macosx","--show-sdk-path"]),
            ("metal-version",["xcrun","--sdk","macosx","metal","--version"]),("swift-version",["xcrun","--sdk","macosx","swiftc","--version"])):
            _,text=invoke(name,argv,"toolchain.txt");metadata[name]=redact(text)
        metadata["actual_os_build"]=metadata["os-build"]
        need(metadata["actual_os_build"]==metadata["expected_os_build"] and metadata["actual_os_build"] in BUILDS.values(),"OS_build_drift_or_missing_pin")
        need(metadata["runner_label"]=="local" or BUILDS.get(metadata["runner_label"])==metadata["actual_os_build"],"runner_label_build")
        need(metadata["architecture"] in ("arm64","x86_64"),"architecture")
        metadata["tool_binaries"]={}
        for tool in ("metal","metallib","swiftc"):
            _,path=invoke("locate-"+tool,["xcrun","--sdk","macosx","-f",tool],"toolchain.txt")
            metadata["tool_binaries"][tool]=dict(path=redact(path),sha256=sha(Path(path).read_bytes()))
        env["NATIVE_OS_BUILD"]=metadata["actual_os_build"]
        invoke("self-test",[sys.executable,"-B","scripts/signed16/probe.py","self-test"],"self-test.txt")
        swift=["xcrun","--sdk","macosx","swiftc","-swift-version","5","-O","-target",metadata["architecture"]+"-apple-macos14.0",
               "-module-cache-path","out-signed16/.build/modules","scripts/signed16/probe.swift","-o","out-signed16/.build/probe"]
        invoke("swift-build",swift);metadata["swift_compile_argv"]=swift
        validation={}
        for mode,flag in MODES.items():
            try:
                air=out/(mode+".air");ll=out/(mode+".ll");lib=out/(mode+".metallib")
                air_arg="out-signed16/"+mode+".air";ir_arg="out-signed16/"+mode+".ll"
                compile_argv=["xcrun","--sdk","macosx","metal","-std=metal3.1","-O2","-g0",flag,"-c","scripts/signed16/probe.metal","-o",air_arg]
                invoke(mode+"-compile",compile_argv);original=sha(air.read_bytes())
                inspect_rc=99;route="unavailable"
                found,_=invoke(mode+"-find-llvm-dis",["xcrun","--sdk","macosx","--find","llvm-dis"],required=False)
                if found==0:
                    inspect_rc,_=invoke(mode+"-llvm-dis",["xcrun","--sdk","macosx","llvm-dis",air_arg,"-o",ir_arg],required=False)
                    if inspect_rc==0: route="llvm-dis-exact-air"
                if inspect_rc!=0:
                    inspect_rc,_=invoke(mode+"-IR-reemit",["xcrun","--sdk","macosx","metal","-x","ir","-S","-emit-llvm","-O0",flag,air_arg,"-o",ir_arg])
                    route="metal-ir-reemit-O0-from-exact-air"
                need(sha(air.read_bytes())==original,"AIR_mutated_during_inspection")
                try: proof=inspect_ir(ll.read_bytes())
                except ValueError as e:
                    proof=dict(qualified=False,ir_sha256=sha(ll.read_bytes()),failure=str(e))
                write_json(out/(mode+".ir-proof.json"),proof)
                link=["xcrun","--sdk","macosx","metallib",air_arg,"-o","out-signed16/"+mode+".metallib"]
                invoke(mode+"-link",link);need(sha(air.read_bytes())==original,"AIR_mutated_during_link")
                pair=dict(mode=mode,source_sha256=sha((here/"probe.metal").read_bytes()),air_sha256=original,ir_sha256=sha(ll.read_bytes()),
                    proof_sha256=sha((out/(mode+".ir-proof.json")).read_bytes()),library_sha256=sha(lib.read_bytes()),
                    compile_exit=0,inspect_exit=inspect_rc,link_exit=0,link_input=mode+".air",inspection_route=route,
                    compile_argv=compile_argv,link_argv=link,fast_math_requested=mode=="fast",sibling_source_ir_used=False,
                    inspection_scope="exact AIR input; IR re-emission if llvm-dis unavailable, not hardware ISA")
                write_json(out/(mode+".compile.json"),pair)
                need(proof["qualified"],"exact_live_conversion_calls_not_proven")
                metadata["native_started"]=True;write_json(out/"metadata.json",metadata)
                rc,_=invoke("native-"+mode,["out-signed16/.build/probe",mode,"scripts/signed16","out-signed16"],mode+".native.txt",required=False)
                need(rc==0,"native_process_failed")
                validation[mode]=validate_mode(out,mode);status["native_modes"][mode]="OBSERVED"
            except (ValueError,OSError,KeyError) as e:
                validation[mode]=dict(status="NOT_QUALIFIED",reason=str(e) if isinstance(e,ValueError) else type(e).__name__)
                status["native_modes"][mode]="NOT_QUALIFIED"
        both=all(status["native_modes"].get(m)=="OBSERVED" for m in MODES)
        equal=both and (out/"precise.gpu.bin").read_bytes()==(out/"fast.gpu.bin").read_bytes()
        write_json(out/"validation.json",dict(modes=validation,cross_mode_native_raw_equal=equal,CPU_expected_outputs=False,
            numeric_confirmation="Main independent review of raw i16/f32 observations; no model-generated output files"))
        need(both and equal,"mode_proof_capture_control_or_cross_mode_failure")
        status.update(status="EXACT_CALL_NATIVE_OBSERVATIONS_READY_FOR_NUMERIC_REVIEW",exit=0)
    except (ValueError,OSError,KeyError) as e:
        status["failure"]=str(e) if isinstance(e,ValueError) else type(e).__name__
    finally:
        status["finished_utc"]=utc();write_json(out/"metadata.json",metadata);write_json(out/"commands.json",commands);write_json(out/"run-status.json",status)
        try: seal(out)
        except (ValueError,OSError) as e:
            status.update(status="PACKAGING_FAILED",exit=3,packaging_failure=type(e).__name__);write_json(out/"run-status.json",status)
        print(encoded(status).decode())
    return status["exit"]


def main():
    parser=argparse.ArgumentParser();parser.add_argument("command",choices=("run","verify","inspect","self-test"));parser.add_argument("path",nargs="?",type=Path)
    args=parser.parse_args()
    if args.command=="self-test":
        result=unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests));return 0 if result.wasSuccessful() else 3
    if args.command=="verify": print(encoded(verify(args.path or Path("out-signed16"))).decode());return 0
    if args.command=="inspect": need(args.path is not None,"IR_path_required");print(encoded(inspect_ir(args.path.read_bytes())).decode());return 0
    need(args.path is None,"run_has_no_optional_path");return run()


if __name__=="__main__": raise SystemExit(main())
