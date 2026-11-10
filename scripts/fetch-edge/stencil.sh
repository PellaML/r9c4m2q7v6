#!/bin/bash
# One bounded owned experiment. No installs, downloads, fallback GPU, or cleanup.
set -euo pipefail
umask 077
here="$(cd "$(dirname "$0")" && pwd)"
if [[ "${1:-}" == --check-artifacts ]]; then
  exec python3 -B - "${2:?output root required}" <<'PY_BOUNDS'
import json, sys
from pathlib import Path
r=Path(sys.argv[1])
paths=[r/'command-exit.json',r/'run-receipt.json',r/'source-sha256.txt',*sorted((r/'results').glob('*'))]
files=[]
for p in paths:
    if not p.exists():continue
    assert p.is_file() and not p.is_symlink() and p.suffix in {'.json','.txt','.bin','.tsv'},str(p)
    assert p.stat().st_size<=1_048_576,str(p)
    files.append(p)
assert len(files)<=64 and sum(p.stat().st_size for p in files)<=4_194_304
assert (r/'command-exit.json').is_file() and (r/'run-receipt.json').is_file()
print(json.dumps({'bounded':True,'files':len(files),'bytes':sum(p.stat().st_size for p in files)}))
PY_BOUNDS
fi
out="${1:?new output root required}"
[[ ! -e "$out" && ! -L "$out" ]] || exit 64
mkdir -p "$out/build"
export PYTHONDONTWRITEBYTECODE=1
python3 -B - "$out" <<'PY_RECEIPT'
import json, platform, subprocess, sys
from pathlib import Path
def run(*a):
    p=subprocess.run(a,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    return {'argv':list(a),'exit':p.returncode,'output':p.stdout[:8192].strip()}
r={'schema':1,'machine':platform.machine(),'os':run('sw_vers'),
   'sdk':run('xcrun','--show-sdk-version'),'xcode':run('xcodebuild','-version'),
   'swift':run('swiftc','--version'),'rust':run('rustc','--version'),
   'source_commit':run('git','rev-parse','HEAD'),'runner_label_is_not_build_evidence':True}
Path(sys.argv[1],'run-receipt.json').write_text(json.dumps(r,indent=2)+'\n')
PY_RECEIPT
for name in stencil.metal stencil.swift stencil-check.rs stencil.sh; do
  (cd "$here" && shasum -a 256 "$name")
done > "$out/source-sha256.txt"
build=99; native=99; validate=99; check=99; negative=99
receipt(){ printf '{"build_exit":%s,"native_exit":%s,"validate_exit":%s,"model_exit":%s,"negative_model_exit":%s}\n' "$build" "$native" "$validate" "$check" "$negative" > "$out/command-exit.json"; }
trap receipt EXIT
set +e
rustc --edition=2024 -O "$here/stencil-check.rs" -o "$out/build/check" > "$out/build/rust-build.txt" 2>&1
build=$?
set -e
[[ $build -eq 0 ]] || exit "$build"
"$out/build/check" prepare "$out/results" > "$out/build/prepare.txt" 2>&1
cp "$out/build/rust-build.txt" "$out/results/rust-build.txt"
"$out/build/check" --self-test > "$out/results/model-self-test.txt" 2>&1
set +e
swiftc -swift-version 5 -O "$here/stencil.swift" -o "$out/build/native" > "$out/results/swift-build.txt" 2>&1
build=$?
set -e
[[ $build -eq 0 ]] || exit "$build"
set +e
"$out/build/native" "$here/stencil.metal" "$out/results" > "$out/results/native-stdout.txt" 2> "$out/results/native-stderr.txt"
native=$?
set -e
[[ $native -eq 0 ]] || exit "$native"
set +e
python3 -B - "$out/results" "$here" > "$out/results/receipt-validation.txt" 2>&1 <<'PY_VALIDATE'
import hashlib,json,re,sys
from pathlib import Path
r=Path(sys.argv[1]);src=Path(sys.argv[2]);sha=lambda b:hashlib.sha256(b).hexdigest()
j=json.loads((r/'native.json').read_text())
assert j['schema']==1 and j['status']=='NATIVE_OBSERVATIONS_COMPLETE'
assert re.fullmatch(r'[0-9]{2}[A-Z][0-9]+[a-z]?',j['os_build'])
assert j['os_build'] in j['os_version_and_build']
assert 'paravirtual' in j['device']['name'].lower()
assert j['compiler']['effective_fast_math'] is True
assert j['source_sha256']==sha((src/'stencil.metal').read_bytes())
assert j['case_manifest_sha256']==sha((r/'cases.tsv').read_bytes())
assert j['seed_sha256']==sha((r/'seed-bgra.bin').read_bytes())
assert j['query_calibration_passed'] is True and len(j['commands'])==27 and len(j['cases'])==13
assert all(c['committed'] and c['wait_returned'] and c['completed'] and c['status']==4 and not c['error_present'] for c in j['commands'])
assert j['draw_count']==11533
assert set(j['output_sha256'])=={'query-calibration.bin'}|{f'case{i:02}.bin' for i in range(13)}
for name,h in j['output_sha256'].items():
    b=(r/name).read_bytes();assert len(b)==(8704 if name=='query-calibration.bin' else 43520 if int(name[4:6])>=10 else 68608) and sha(b)==h
for i,c in enumerate(j['cases']):
    assert c['id']==i and c['guards_checked'] and c['samples']==1 and not c['depth_attachment'] and not c['depth_write']
    assert c['logical_strip_draws']==c['physical_consumer_draw_calls']==1 and not c['split_triangle_calls'] and c['discard_even_x']==0
    assert c['color0_write_mask']==15 and c['same_consumer_encoder'] and not c['all_MRT_blending']
    d=c['state_descriptor'];assert d['depth_compare']==7 and d['depth_write'] is False and d['front']==d['back']
    assert c['consumer_stencil_attached'] is True and c['explicit_depth_stencil_state_bound'] is True
    enabled=i not in (0,5,10)
    assert c['stencil_tests_enabled'] is enabled
    if enabled:assert d['front']['fail']==d['front']['depth_fail']==0
    else:assert d['front'] is None and d['back'] is None
    visible=i>=10
    assert c['visible_color_derivative_control'] is visible and c['no_color2_attachment'] is visible
    assert c['color_attachment_formats']==([80,65] if visible else [80,125,125])
    assert c['color_attachment_write_masks']==([15,0] if visible else [15,15,15])
    pd=c['pipeline_descriptor_getters']
    assert pd['formats0to2']==([80,65,0] if visible else [80,125,125])
    assert pd['write_masks0to2'][:2]==([15,0] if visible else [15,15])
    assert pd['depth_format']==0 and pd['samples']==1
    assert pd['stencil_format']==253
    if visible:assert pd['fragment']=='stencil_visible_early'
    assert c['RG16_clear_half_words']==([0x3400,0x3a00] if visible else [])
    assert [t['format'] for t in c['actual_textures']]==([80,65,253,73] if visible else [80,125,125,253,73])
    for t in c['actual_textures']:
        assert (t['width'],t['height'],t['samples'],t['mips'],t['array_length'],t['storage'],t['usage'],t['type'],t['framebuffer_only'])==(32,32,1,1,1,2,4,2,False)
print(json.dumps({'native_receipts_valid':True,'actual_os_build':j['os_build'],'device':j['device'],'compiler':j['compiler'],'no_qualification_from_filename':True}))
PY_VALIDATE
validate=$?
"$out/build/check" check "$out/results" > "$out/results/model-check.txt" 2>&1
check=$?
"$out/build/check" check-wrong "$out/results" > "$out/results/negative-check.txt" 2>&1
negative=$?
set -e
receipt
[[ $validate -eq 0 && $check -eq 0 && $negative -eq 3 ]] || exit 3
python3 -B - "$out/results" <<'PY_NEGATIVE'
import json,sys
from pathlib import Path
r=Path(sys.argv[1]);good=json.loads((r/'model-check.json').read_text());bad=json.loads((r/'negative-check.json').read_text())
assert good['native_model_qualification'] is True and bad['native_model_qualification'] is False
assert bad['deliberately_wrong_ignore_stencil_model'] is True
assert any(x['color_byte_mismatch']>0 for x in bad['rows'])
print('EXACT_NATIVE_MODEL_AND_WRONG_MODEL_REJECTION_CONFIRMED')
PY_NEGATIVE
