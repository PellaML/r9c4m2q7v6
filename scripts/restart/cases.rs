// Synthetic source inputs and exact GPU-to-GPU comparisons. No CPU image oracle.
#![forbid(unsafe_code)]
use std::{env, fs, io::Write, path::Path};
const MARK: u32 = 0xffff;
const W: usize = 34;
const H: usize = 18;
const PITCH: usize = 256;
const G: usize = 256;
const LEN: usize = G * 2 + H * PITCH;
const CLEAR: [u8; 4] = [145, 162, 179, 196];
const XY: [[f32; 2]; 15] = [
    [2., 2.],
    [2., 6.],
    [6., 2.],
    [6., 6.],
    [8., 2.],
    [12., 2.],
    [12., 6.],
    [16., 2.],
    [16., 6.],
    [22., 2.],
    [22., 6.],
    [26., 2.],
    [26., 6.],
    [8., 6.],
    [10., 6.],
];
#[derive(Clone)]
struct Draw {
    primitive: u8,
    bits: u8,
    offset: usize,
    count: usize,
    instances: u8,
    base: u8,
}
struct Capture {
    name: String,
    draws: Vec<Draw>,
    skip: u32,
    foreground: bool,
}
struct Plan {
    captures: Vec<Capture>,
    indices: Vec<u8>,
    equal: Vec<(usize, usize)>,
    negative: Vec<(usize, usize, &'static str)>,
    lists: Vec<(String, [usize; 5])>,
}
impl Plan {
    fn draw(&mut self, primitive: u8, bits: u8, ids: &[u32]) -> Vec<Draw> {
        if ids.is_empty() {
            return vec![];
        }
        while self.indices.len() % 4 != 0 {
            self.indices.push(0xcd);
        }
        let offset = self.indices.len();
        for &id in ids {
            assert!(id <= 14 || id == 65534 || id == MARK);
            self.indices.extend_from_slice(&id.to_le_bytes()[..usize::from(bits / 8)]);
        }
        vec![Draw { primitive, bits, offset, count: ids.len(), instances: 2, base: 0 }]
    }
    fn capture(&mut self, name: &str, draws: Vec<Draw>, foreground: bool, skip: u32) -> usize {
        let id = self.captures.len();
        self.captures.push(Capture { name: name.into(), draws, skip, foreground });
        id
    }
    fn single(&mut self, name: &str, primitive: u8, bits: u8, ids: &[u32], fg: bool) -> usize {
        let draws = self.draw(primitive, bits, ids);
        self.capture(name, draws, fg, u32::MAX)
    }
}
fn runs(ids: &[u32]) -> Vec<&[u32]> {
    ids.split(|&v| v == MARK).filter(|r| r.len() >= 3).collect()
}
fn strip_triangles(ids: &[u32], carry: bool) -> Vec<u32> {
    let mut result = Vec::new();
    let mut parity = 0;
    for r in runs(ids) {
        if !carry {
            parity = 0;
        }
        for w in r.windows(3) {
            result.extend(if parity % 2 == 0 { [w[0], w[1], w[2]] } else { [w[1], w[0], w[2]] });
            parity += 1;
        }
    }
    result
}
fn plan() -> Plan {
    let mut p = Plan { captures: vec![], indices: vec![], equal: vec![], negative: vec![], lists: vec![] };
    p.capture("gpu-seed", vec![], false, u32::MAX);
    for (name, bits, special, direct) in [("max-u32", 32, MARK, 13), ("below-max-u16", 16, MARK - 1, 14)] {
        let a = p.single(name, 3, bits, &[0, 1, special], true);
        let b = p.single(&format!("{name}-alias"), 3, 16, &[0, 1, direct], true);
        p.equal.push((a, b));
    }
    let a = p.single("list-clean-u16", 3, 16, &[0, 1, 2, 5, 6, 7], true);
    let b = p.single("list-clean-u32", 3, 32, &[0, 1, 2, 5, 6, 7], true);
    p.equal.push((a, b));
    let a = p.single("strip-clean", 4, 16, &[0, 1, 2, 3, 4], true);
    let b = p.single("strip-clean-triangles", 3, 16, &strip_triangles(&[0, 1, 2, 3, 4], false), true);
    p.equal.push((a, b));
    let fixtures: [(&str, &[u32]); 7] = [
        ("odd3", &[0, 1, 2, MARK, 5, 6, 7, 8]),
        ("even4", &[0, 1, 2, 3, MARK, 5, 6, 7, 8]),
        ("odd5", &[0, 1, 2, 3, 4, MARK, 5, 6, 7, 8]),
        ("boundaries", &[MARK, MARK, 0, 1, 2, MARK, MARK, 5, 6, 7, 8, MARK, MARK]),
        ("incomplete", &[0, MARK, 1, 2, MARK, 5, 6, 7, MARK, 9, MARK, 10, 11, MARK]),
        ("incomplete-only", &[MARK, 0, MARK, 1, 2, MARK, MARK]),
        ("markers-only", &[MARK, MARK, MARK]),
    ];
    for (name, ids) in fixtures {
        let r = runs(ids);
        let fg = !r.is_empty();
        let a = p.single(&format!("strip-{name}"), 4, 16, ids, fg);
        let mut reset = Vec::new();
        for run in r {
            reset.extend(p.draw(4, 16, run));
        }
        let b = p.capture(&format!("strip-{name}-fresh-draws"), reset.clone(), fg, u32::MAX);
        p.equal.push((a, b));
        if !fg {
            p.equal.push((a, 0));
        }
        if name == "odd3" || name == "odd5" {
            let c = p.single(&format!("negative-{name}-carry-parity"), 3, 16, &strip_triangles(ids, true), true);
            p.negative.push((b, c, "flip-group-two"));
        }
        if name == "odd3" {
            let mut individual = Vec::new();
            for base in 0..2 {
                for d in &reset {
                    let mut d = d.clone();
                    d.instances = 1;
                    d.base = base;
                    individual.push(d);
                }
            }
            let c = p.capture("strip-individual-instances", individual, true, u32::MAX);
            p.equal.push((b, c));
            let c = p.capture("negative-discard-row", reset.clone(), true, 5);
            p.negative.push((b, c, "discard-row-five"));
            let one = reset
                .iter()
                .cloned()
                .map(|mut d| {
                    d.instances = 1;
                    d
                })
                .collect();
            let c = p.capture("negative-one-instance", one, true, u32::MAX);
            p.negative.push((b, c, "omit-instance-two"));
        }
    }
    // A separate classification experiment, not an assumption about any workload.
    let lists: [(&str, &[u32]); 4] = [
        ("first", &[MARK, 0, 1, 2, 5, 6, 7, 9, 10]),
        ("middle", &[0, MARK, 1, 5, 6, 7, 9, 10, 11]),
        ("third", &[0, 1, MARK, 5, 6, 7]),
        ("boundaries", &[MARK, 0, 1, MARK, MARK, 5, 6, 7, MARK, 9, 10, MARK]),
    ];
    for (name, ids) in lists {
        let actual = p.single(&format!("list-{name}-u16"), 3, 16, ids, false);
        let regular = p.single(&format!("list-{name}-regular-u32"), 3, 32, ids, false);
        let dropped: Vec<_> = ids.chunks_exact(3).filter(|t| !t.contains(&MARK)).flatten().copied().collect();
        let drop_tri = p.single(&format!("list-{name}-drop-triangles"), 3, 16, &dropped, false);
        let reset: Vec<_> = runs(ids).into_iter().flat_map(|r| r.chunks_exact(3).flatten().copied()).collect();
        let reset_tri = p.single(&format!("list-{name}-reset-groups"), 3, 16, &reset, false);
        let mut filtered: Vec<_> = ids.iter().copied().filter(|&v| v != MARK).collect();
        filtered.truncate(filtered.len() / 3 * 3);
        let drop_index = p.single(&format!("list-{name}-drop-indices"), 3, 16, &filtered, false);
        p.lists.push((name.into(), [actual, regular, drop_tri, reset_tri, drop_index]));
    }
    p
}
fn vertices() -> Vec<u8> {
    XY.into_iter()
        .flat_map(|[x, y]| [x / 16. - 1., 1. - y / 8., 0.5, 1.])
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect()
}
fn seed() -> Vec<u8> {
    let mut b = vec![0xcd; LEN];
    for y in 0..H {
        for x in 0..W {
            let o = G + y * PITCH + x * 4;
            b[o..o + 4].copy_from_slice(&CLEAR);
        }
    }
    b
}
fn manifest(p: &Plan) -> String {
    let captures=p.captures.iter().enumerate().map(|(id,c)| {
        let draws=c.draws.iter().map(|d| format!("{{\"primitive\":{},\"indexBits\":{},\"offset\":{},\"count\":{},\"instances\":{},\"baseInstance\":{}}}",d.primitive,d.bits,d.offset,d.count,d.instances,d.base)).collect::<Vec<_>>().join(",");
        format!("{{\"id\":{id},\"name\":\"{}\",\"skipRow\":{},\"requireForeground\":{},\"draws\":[{draws}]}}",c.name,c.skip,c.foreground)
    }).collect::<Vec<_>>().join(",\n");
    let equals = p.equal.iter().map(|(a, b)| format!("[{a},{b}]")).collect::<Vec<_>>().join(",");
    let negatives = p
        .negative
        .iter()
        .map(|(a, b, k)| format!("{{\"control\":{a},\"negative\":{b},\"kind\":\"{k}\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    let lists =
        p.lists.iter().map(|(n, v)| format!("{{\"name\":\"{n}\",\"captures\":{v:?}}}")).collect::<Vec<_>>().join(",");
    format!("{{\"schema\":1,\"width\":34,\"height\":18,\"pitch\":256,\"guardBytes\":256,\"viewport\":[1,1,32,16,0,1],\"scissor\":[1,1,32,16],\"vertexLayout\":\"15 little-endian uint4 position bit patterns; uint vertex_id 65535 aliases slot13 and 65534 aliases slot14; all IDs valid\",\"instanceMap\":\"clip.y -= instance_id; two disjoint 8-pixel bands; baseVertex=0\",\"rgba\":\"R=front_facing bit; G=160+flat vertex group (1..4); B=instance_id+1; A=215\",\"frontWinding\":\"explicit clockwise\",\"samples\":1,\"cpu_expected_images\":false,\"indexBytes\":{},\"captureCount\":{},\"captures\":[{captures}],\"equal\":[{equals}],\"negative\":[{negatives}],\"listModels\":[\"native-u16\",\"regular-u32\",\"drop-triangles\",\"reset-groups\",\"drop-indices\"],\"lists\":[{lists}]}}\n",p.indices.len(),p.captures.len())
}
fn inputs(p: &Plan) -> Vec<(&str, Vec<u8>)> {
    vec![
        ("cases.json", manifest(p).into_bytes()),
        ("vertices.bin", vertices()),
        ("indices.bin", p.indices.clone()),
        ("seed.bin", seed()),
    ]
}
fn save(path: &Path, b: &[u8]) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(b))
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn read(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("InputsMissing {}: {e}", path.display()))?;
    if !m.is_file() || m.len() > max as u64 {
        return Err("input type/size".into());
    }
    fs::read(path).map_err(|e| e.to_string())
}
fn generate(dir: &Path) -> Result<(), String> {
    fs::create_dir(dir).map_err(|e| format!("fresh directory required: {e}"))?;
    for (name, b) in inputs(&plan()) {
        save(&dir.join(name), &b)?;
    }
    Ok(())
}
fn inspect(b: &[u8]) -> Result<(usize, u8), String> {
    if b.len() != LEN {
        return Err("raw length".into());
    }
    if b[..G].iter().chain(b[G + H * PITCH..].iter()).any(|&v| v != 0xcd) {
        return Err("prefix/suffix guard".into());
    }
    let mut count = 0;
    let mut instances = 0;
    for y in 0..H {
        let o = G + y * PITCH;
        if b[o + W * 4..o + PITCH].iter().any(|&v| v != 0xcd) {
            return Err("row padding guard".into());
        }
        for x in 0..W {
            let q = &b[o + x * 4..o + x * 4 + 4];
            if q == CLEAR {
                continue;
            }
            if x == 0
                || x == W - 1
                || y == 0
                || y == H - 1
                || q[0] > 1
                || !(161..=164).contains(&q[1])
                || !(1..=2).contains(&q[2])
                || q[3] != 215
            {
                return Err(format!("pixel/marker/guard {x},{y}"));
            }
            if (q[2] == 1 && y >= 9) || (q[2] == 2 && y < 9) {
                return Err("instance spatial band".into());
            }
            count += 1;
            instances |= 1 << (q[2] - 1);
        }
    }
    Ok((count, instances))
}
fn negative(a: &[u8], b: &[u8], kind: &str) -> Result<usize, String> {
    let mut changes = 0;
    for y in 0..H {
        for x in 0..W {
            let o = G + y * PITCH + x * 4;
            let mut expected = [a[o], a[o + 1], a[o + 2], a[o + 3]];
            if expected != CLEAR {
                match kind {
                    "flip-group-two" if expected[1] == 162 => expected[0] ^= 1,
                    "discard-row-five" if y == 5 => expected = CLEAR,
                    "omit-instance-two" if expected[2] == 2 => expected = CLEAR,
                    _ => (),
                }
            }
            changes += usize::from(a[o..o + 4] != expected);
            if b[o..o + 4] != expected {
                return Err(format!("genuine negative mismatch: {kind}, {x},{y}"));
            }
        }
    }
    if changes == 0 {
        return Err(format!("vacuous negative {kind}"));
    }
    Ok(changes)
}
fn validate(dir: &Path) -> Result<(), String> {
    let p = plan();
    for (name, b) in inputs(&p) {
        if read(&dir.join(name), b.len())? != b {
            return Err(format!("changed input {name}"));
        }
    }
    let receipt = String::from_utf8(read(&dir.join("gpu-commands.tsv"), 65536)?).map_err(|e| e.to_string())?;
    let rows: Vec<_> = receipt.lines().collect();
    if rows.len() != p.captures.len() + 1
        || rows[0] != "id\tname\tstatus\terror_present\tdraw_calls\tgpu_start\tgpu_end"
    {
        return Err("GPU receipt coverage".into());
    }
    let mut draw_receipt = String::from(
        "capture\tdraw\tprimitive\tindex_bits\tindex_offset\tindex_count\tinstances\tbase_instance\tbase_vertex\n",
    );
    let mut images = Vec::new();
    let mut observations = String::from("id\tname\tcovered_pixels\tinstance_mask\n");
    let mut pixels = String::from("id\tx\ty\tr\tg\tb\ta\n");
    for (i, c) in p.captures.iter().enumerate() {
        let f: Vec<_> = rows[i + 1].split('\t').collect();
        if f.len() != 7
            || f[0] != i.to_string()
            || f[1] != c.name
            || f[2] != "4"
            || f[3] != "0"
            || f[4] != c.draws.len().to_string()
        {
            return Err(format!("GPU completion {i}"));
        }
        for (j, d) in c.draws.iter().enumerate() {
            draw_receipt += &format!(
                "{i}\t{j}\t{}\t{}\t{}\t{}\t{}\t{}\t0\n",
                d.primitive,
                d.bits,
                64 + d.offset,
                d.count,
                d.instances,
                d.base
            );
        }
        let b = read(&dir.join(format!("case-{i}.gpu.bin")), LEN)?;
        let (n, mask) = inspect(&b)?;
        let required = if c.name == "negative-one-instance" { 1 } else { 3 };
        if c.foreground && (n == 0 || mask != required) {
            return Err(format!("missing positive coverage/instance {i}"));
        }
        observations += &format!("{i}\t{}\t{n}\t{mask}\n", c.name);
        for y in 0..H {
            for x in 0..W {
                let q = &b[G + y * PITCH + x * 4..][..4];
                pixels += &format!("{i}\t{x}\t{y}\t{}\t{}\t{}\t{}\n", q[0], q[1], q[2], q[3]);
            }
        }
        images.push(b);
    }
    if read(&dir.join("draws.tsv"), 65536)? != draw_receipt.as_bytes() {
        return Err("actual encoded draw receipt mismatch".into());
    }
    if images[0] != seed() {
        return Err("GPU seed roundtrip".into());
    }
    for &(a, b) in &p.equal {
        if images[a] != images[b] {
            return Err(format!("exact GPU positive differs: {} vs {}", p.captures[a].name, p.captures[b].name));
        }
    }
    let mut checks = String::from("control\tnegative\tkind\tchanged_pixels\n");
    for &(a, b, k) in &p.negative {
        let n = negative(&images[a], &images[b], k)?;
        checks += &format!("{a}\t{b}\t{k}\t{n}\n");
    }
    let models = ["regular-u32", "drop-triangles", "reset-groups", "drop-indices"];
    let mut matches = String::from("case\tmodel\texact_gpu_match\n");
    let mut winners = Vec::new();
    for (j, model) in models.iter().enumerate() {
        let mut all = true;
        for (name, v) in &p.lists {
            let same = images[v[0]] == images[v[j + 1]];
            all &= same;
            matches += &format!("{name}\t{model}\t{same}\n");
        }
        if all {
            winners.push(*model);
        }
    }
    save(&dir.join("observations.tsv"), observations.as_bytes())?;
    save(&dir.join("pixels.tsv"), pixels.as_bytes())?;
    save(&dir.join("negative-controls.tsv"), checks.as_bytes())?;
    save(&dir.join("list-models.tsv"), matches.as_bytes())?;
    for a in 1..5 {
        for b in a + 1..5 {
            if p.lists.iter().all(|(_, v)| images[v[a]] == images[v[b]]) {
                return Err("list models are not discriminating".into());
            }
        }
    }
    if winners.len() != 1 {
        return Err(format!("list semantics unresolved: {winners:?}"));
    }
    save(&dir.join("validation.json"),format!("{{\"status\":\"OBSERVED_VALIDATED\",\"gpu_captures\":{},\"exact_positive_pairs\":{},\"genuine_gpu_negatives\":{},\"strip_restart_resets_winding\":true,\"incomplete_runs_emit_no_triangles\":true,\"list_observed_model\":\"{}\",\"cpu_image_oracle\":false,\"scope\":\"retained source, positions, inputs, OS/SDK and device only\"}}\n",p.captures.len(),p.equal.len(),p.negative.len(),winners[0]).as_bytes())?;
    print!("{observations}{checks}{matches}");
    Ok(())
}
fn main() {
    let a: Vec<_> = env::args().collect();
    let r = match a.as_slice() {
        [_, op, dir] if op == "generate" => generate(Path::new(dir)),
        [_, op, dir] if op == "validate" => validate(Path::new(dir)),
        _ => Err("usage: cases generate NEW_DIRECTORY | validate CAPTURE_DIRECTORY".into()),
    };
    if let Err(e) = r {
        eprintln!("NOT_QUALIFIED: {e}");
        std::process::exit(3);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_runs_discriminate_parity() {
        for n in [3, 5] {
            let mut v: Vec<_> = (0..n).collect();
            v.extend([MARK, 5, 6, 7, 8]);
            assert_ne!(strip_triangles(&v, false), strip_triangles(&v, true));
        }
    }
    #[test]
    fn even_run_does_not_flip_next_run() {
        let v = [0, 1, 2, 3, MARK, 5, 6, 7, 8];
        assert_eq!(strip_triangles(&v, false), strip_triangles(&v, true));
    }
    #[test]
    fn incomplete_and_empty_runs_produce_no_triangle_inputs() {
        assert!(strip_triangles(&[MARK, 0, MARK, 1, 2, MARK, MARK], false).is_empty());
    }
    #[test]
    fn all_draw_indices_are_bounded_and_aligned() {
        let p = plan();
        for c in p.captures {
            for d in c.draws {
                assert_eq!(d.offset % 4, 0);
                assert!(d.offset + d.count * usize::from(d.bits / 8) <= p.indices.len());
                assert!(d.instances + d.base <= 2);
            }
        }
    }
    #[test]
    fn unique_small_plan_and_four_gpu_negatives() {
        let p = plan();
        let mut n = p.captures.iter().map(|c| &c.name).collect::<Vec<_>>();
        n.sort();
        n.dedup();
        assert_eq!(n.len(), p.captures.len());
        assert!(n.len() < 64);
        assert_eq!(p.negative.len(), 4);
        assert_eq!(p.lists.len(), 4);
    }
    #[test]
    fn seed_has_no_foreground_and_wrong_length_refuses() {
        assert_eq!(inspect(&seed()).unwrap(), (0, 0));
        assert!(inspect(&seed()[..LEN - 1]).is_err());
    }
    #[test]
    fn source_vertices_are_exact_binary_fractions() {
        assert_eq!(vertices().len(), 240);
        assert_eq!(&vertices()[..4], &(-0.875f32).to_bits().to_le_bytes());
    }
    #[test]
    fn generator_has_no_expected_gpu_image() {
        assert!(manifest(&plan()).contains("\"cpu_expected_images\":false"));
        assert_eq!(inputs(&plan()).len(), 4);
    }
}
