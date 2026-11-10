#!/bin/bash
# One bounded owned experiment. No installs, downloads, retries or cleanup.
set -euo pipefail
umask 077
here="$(cd "$(dirname "$0")" && pwd)"
if [[ "${1:-}" == --check-artifacts ]]; then
  exec python3 -B - "${2:?output root required}" <<'PY_BOUNDS'
import json,sys
from pathlib import Path
r=Path(sys.argv[1])
top={'command-exit.json','run-receipt.json','source-sha256.txt'}
names={'cases.tsv','seed-rgba16.bin','seed-rg16.bin','query-calibration.bin','native.json',
       'model-check.json','negative-check.json','model-check.txt','negative-check.txt',
       'receipt-validation.txt','rust-build.txt','swift-build.txt','model-self-test.txt',
       'native-stdout.txt','native-stderr.txt'}|{f'case{i:02}.bin' for i in range(8)}
paths=[*(r/name for name in sorted(top)),*sorted((r/'results').glob('*'))]
files=[]
for p in paths:
    if not p.exists():continue
    assert p.is_file() and not p.is_symlink()
    assert p.name in (top if p.parent==r else names),p.name
    assert p.stat().st_size<=1_048_576,p.name
    files.append(p)
assert len(files)<=32 and sum(p.stat().st_size for p in files)<=4_194_304
assert (r/'command-exit.json').is_file() and (r/'run-receipt.json').is_file()
print(json.dumps({'bounded':True,'files':len(files),'bytes':sum(p.stat().st_size for p in files)}))
PY_BOUNDS
fi
out="${1:?new output root required}"
[[ ! -e "$out" && ! -L "$out" ]] || exit 64
mkdir -p "$out/build"
export PYTHONDONTWRITEBYTECODE=1
python3 -B - "$out" <<'PY_RECEIPT'
import json,platform,subprocess,sys
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
for name in odd-edge.metal odd-edge.swift odd-edge-check.rs odd-edge.sh; do
  (cd "$here" && shasum -a 256 "$name")
done > "$out/source-sha256.txt"
build=99; native=99; validate=99; check=99; negative=99
receipt(){ printf '{"build_exit":%s,"native_exit":%s,"validate_exit":%s,"model_exit":%s,"negative_model_exit":%s}\n' "$build" "$native" "$validate" "$check" "$negative" > "$out/command-exit.json"; }
trap receipt EXIT
set +e
rustc --edition=2024 -O "$here/odd-edge-check.rs" -o "$out/build/check" > "$out/build/rust-build.txt" 2>&1
build=$?
set -e
if [[ $build -ne 0 ]]; then
  mkdir -p "$out/results"; cp "$out/build/rust-build.txt" "$out/results/rust-build.txt"; exit "$build"
fi
"$out/build/check" prepare "$out/results"
cp "$out/build/rust-build.txt" "$out/results/rust-build.txt"
"$out/build/check" --self-test > "$out/results/model-self-test.txt" 2>&1
set +e
swiftc -swift-version 5 -O "$here/odd-edge.swift" -o "$out/build/native" > "$out/results/swift-build.txt" 2>&1
build=$?
set -e
[[ $build -eq 0 ]] || exit "$build"
set +e
"$out/build/native" "$here/odd-edge.metal" "$out/results" > "$out/results/native-stdout.txt" 2> "$out/results/native-stderr.txt"
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
assert re.fullmatch(r'[0-9]{2}[A-Z][0-9]+[a-z]?',j['os_build']) and j['os_build'] in j['os_version_and_build']
assert 'paravirtual' in j['device']['name'].lower() and j['compiler']['effective_fast_math'] is True
assert j['source_sha256']==sha((src/'odd-edge.metal').read_bytes())
contract=j['contract']
assert (contract['width'],contract['height'],contract['case_count'],contract['case_bytes'])==(35,27,8,58368)
assert contract['framebuffer_input']=='half2 color(1)' and contract['source_declared_fetch_slots']==[1]
assert contract['derivative_precision']=='float32; half4 color0 return'
assert contract['all_cases_explicit_AlwaysKeep'] is True and contract['all_cases_early_attribute'] is True
assert contract['no_device_buffer_fragment_side_effects'] is True and contract['model_qualification_not_inferred_from_completion'] is True
assert all(contract[k] is False for k in ['shader_depth_output','shader_stencil_output','discard_supported','control_flow_supported',
                                         'modeled_OOB_fetch_values','private_texture_physical_pitch_asserted','invocation_counts_measured'])
for field,name in [('case_manifest_sha256','cases.tsv'),('color_seed_sha256','seed-rgba16.bin'),('fetch_seed_sha256','seed-rg16.bin')]:
    assert j[field]==sha((r/name).read_bytes())
assert j['query_calibration_passed'] is True and j['all8_cases_succeeded'] is True and j['all17_command_buffers_succeeded'] is True
assert len(j['commands'])==17 and len(j['cases'])==8 and j['case_failures']==[]
assert len(j['case_attempts'])==8 and all(c['id']==i and c['outcome']=='captured' and not c['prior_command_error'] for i,c in enumerate(j['case_attempts']))
assert all(c['committed'] and c['wait_returned'] and c['completed'] and c['status']==4 and not c['error_present'] and c['error_options']==1 for c in j['commands'])
assert j['draw_count']==5305 and j['output_bytes']==474368
assert set(j['output_sha256'])=={'query-calibration.bin'}|{f'case{i:02}.bin' for i in range(8)}
for name,h in j['output_sha256'].items():
    b=(r/name).read_bytes();assert len(b)==(7424 if name=='query-calibration.bin' else 58368) and sha(b)==h
keep={'compare':7,'depth_fail':0,'fail':0,'pass':0,'read_mask':4294967295,'write_mask':4294967295}
for field,compare,op,read,write in [('query_state',2,0,255,0),('seed_state',7,2,255,255)]:
    d=j[field];f={'compare':compare,'depth_fail':0,'fail':0,'pass':op,'read_mask':read,'write_mask':write}
    assert d=={'depth_compare':7,'depth_write':False,'front':f,'back':f}
for i,c in enumerate(j['cases']):
    x=[3,25,3,25][i//2];y=[5,5,19,19][i//2];variant=bool(i%2)
    assert c['id']==i and c['guards_checked'] and c['samples']==1 and not c['depth_attachment'] and not c['depth_write']
    assert c['logical_strip_draws']==c['physical_consumer_draw_calls']==1 and not c['split_triangle_calls'] and c['same_consumer_encoder']
    assert c['color0_write_mask']==15 and not c['all_MRT_blending'] and not c['discard'] and not c['control_flow']
    assert c['consumer_stencil_attached'] is True and c['explicit_depth_stencil_state_bound'] is True and c['explicit_both_AlwaysKeep_faces'] is True
    assert c['state_descriptor']=={'depth_compare':7,'depth_write':False,'front':keep,'back':keep}
    assert c['reference_front_back']==117 and c['early_attribute_requested'] is True
    assert c['derivative_variant'] is variant and c['source_framebuffer_fetch_slots']==[1]
    assert c['derivative_operand']=='interpolated varying only'
    assert c['scissor']==[x,y,10,8] and c['points']==[[x,y],[x,y+8],[x+10,y],[x+10,y+8]]
    assert c['viewport']==[0,0,35,27] and c['render_target_size_getters']==[35,27]
    assert c['color_attachment_formats']==[115,65] and c['color_attachment_write_masks']==[15,0] and c['no_color2_attachment'] is True
    assert c['color_load_actions']==c['color_store_actions']==[1,1] and c['stencil_load_action']==c['stencil_store_action']==1
    assert c['readback_pitches']==[512,256,256,512,256,256]
    pd=c['pipeline_descriptor_getters'];assert pd['formats0to2']==[115,65,0] and pd['write_masks0to2'][:2]==[15,0]
    assert pd['depth_format']==0 and pd['stencil_format']==253 and pd['samples']==1 and not pd['alpha_to_coverage'] and not pd['alpha_to_one']
    assert pd['fragment']==c['fragment']==('edge_derivative' if variant else 'edge_scalar')
    assert [t['format'] for t in c['actual_textures']]==[115,65,253,73]
    for t in c['actual_textures']:
        assert (t['width'],t['height'],t['samples'],t['mips'],t['array_length'],t['storage'],t['usage'],t['type'],t['framebuffer_only'])==(35,27,1,1,1,2,4,2,False)
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
assert good['scalar_derivative_pairs_equal'] is True and bad['deliberately_wrong_clamped_geometry_model'] is True
assert all(bad['rows'][i]['color_half_word_mismatch']>0 for i in [3,5,7])
assert all(n>0 for n in good['right_bottom_corner_clamped_geometry_differences'])
assert all(n>0 for n in good['zero_fetch_differences_by_case'])
print('EXACT_NATIVE_MODEL_AND_WRONG_CLAMPED_GEOMETRY_REJECTION_CONFIRMED')
PY_NEGATIVE
