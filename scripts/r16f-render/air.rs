// Bounded host-only container/text inspection. Never recompile MSL for evidence.
#![forbid(unsafe_code)]
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, String>;
fn need(ok: bool, why: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(why.into())
    }
}

pub const ENTRIES: [(&str, u8); 4] = [
    ("triangle", 0),
    ("emit_f32", 1),
    ("emit_f16", 1),
    ("observe", 2),
];

// Small dependency-free SHA-256; known-answer tests below. It is only used for
// file identity, never as a substitute for bounded parsing or native execution.
pub fn digest(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, v) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(v.try_into().unwrap());
        }
        for i in 16..64 {
            let a = w[i - 15];
            let b = w[i - 2];
            w[i] = w[i - 16]
                .wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
                .wrapping_add(w[i - 7])
                .wrapping_add(b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10));
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut z] = h;
        for i in 0..64 {
            let t1 = z
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            z = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (v, add) in h.iter_mut().zip([a, b, c, d, e, f, g, z]) {
            *v = v.wrapping_add(add);
        }
    }
    let mut out = [0; 32];
    for (i, v) in h.into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}
pub fn sha(data: &[u8]) -> String {
    digest(data).iter().map(|v| format!("{v:02x}")).collect()
}

fn slice(b: &[u8], p: usize, n: usize) -> Result<&[u8]> {
    b.get(p..p.checked_add(n).ok_or("range overflow")?)
        .ok_or_else(|| "truncated container".into())
}
fn u32le(b: &[u8], p: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(slice(b, p, 4)?.try_into().unwrap()) as usize)
}
fn u64le(b: &[u8], p: usize) -> Result<usize> {
    usize::try_from(u64::from_le_bytes(slice(b, p, 8)?.try_into().unwrap()))
        .map_err(|_| "host size overflow".into())
}
pub struct Module<'a> {
    pub name: String,
    pub stage: u8,
    pub offset: usize,
    pub bytes: &'a [u8],
}

pub fn extract(b: &[u8]) -> Result<Vec<Module<'_>>> {
    need(
        (88..=1_048_576).contains(&b.len()) && &b[..4] == b"MTLB",
        "metallib bounds/signature",
    )?;
    need(
        b[6..8] == [2, 0] && u64le(b, 16)? == b.len(),
        "container version/declared length",
    )?;
    let (fo, fl, po, pl, qo, ql, bo, bl) = (
        u64le(b, 24)?,
        u64le(b, 32)?,
        u64le(b, 40)?,
        u64le(b, 48)?,
        u64le(b, 56)?,
        u64le(b, 64)?,
        u64le(b, 72)?,
        u64le(b, 80)?,
    );
    // A narrow current-library layout, not a scan for coincidental BC magic.
    for (o, n) in [(fo, fl), (po, pl), (qo, ql), (bo, bl)] {
        slice(b, o, n)?;
    }
    need(
        fo == 88 && po == fo + fl + 8 && qo == po + pl && bo == qo + ql && bo + bl == b.len(),
        "section layout",
    )?;
    let count = u32le(b, fo)?;
    need((1..=8).contains(&count), "module count bound")?;
    let mut pos = fo + 4;
    let mut modules = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..count {
        let len = u32le(b, pos)?;
        need((8..=4096).contains(&len), "function record length")?;
        let record = slice(b, pos, len)?;
        need(pos + len <= po - 4, "function record section")?;
        let mut q = 4;
        let mut tags = BTreeMap::new();
        loop {
            let tag = slice(record, q, 4)?;
            q += 4;
            if tag == b"ENDT" {
                need(q == len, "record trailer")?;
                break;
            }
            need(
                [b"NAME", b"TYPE", b"HASH", b"MDSZ", b"OFFT", b"VERS"]
                    .contains(&tag.try_into().unwrap()),
                "unrecognized record tag",
            )?;
            let n = u16::from_le_bytes(slice(record, q, 2)?.try_into().unwrap()) as usize;
            q += 2;
            let value = slice(record, q, n)?;
            q += n;
            need(tags.insert(tag, value).is_none(), "duplicate record tag")?;
        }
        need(tags.len() == 6, "missing record tags")?;
        let get = |key: &[u8]| {
            tags.get(key)
                .copied()
                .ok_or_else(|| "missing tag".to_string())
        };
        let name = get(b"NAME")?;
        need(
            name.len() >= 2 && name.len() <= 65 && name.last() == Some(&0),
            "entry name length/terminator",
        )?;
        need(
            name[..name.len() - 1]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'_'),
            "entry name characters",
        )?;
        let name =
            String::from_utf8(name[..name.len() - 1].to_vec()).map_err(|_| "entry encoding")?;
        need(names.insert(name.clone()), "duplicate entry name")?;
        let stage = get(b"TYPE")?;
        need(stage.len() == 1 && stage[0] <= 2, "entry stage")?;
        let size = get(b"MDSZ")?;
        let offsets = get(b"OFFT")?;
        need(
            size.len() == 8 && offsets.len() == 24 && get(b"VERS")?.len() == 8,
            "module fields",
        )?;
        let size = u64le(size, 0)?;
        let relative = u64le(offsets, 16)?;
        need(
            u64le(offsets, 0)? < pl && u64le(offsets, 8)? < ql,
            "metadata offsets",
        )?;
        need(
            size >= 32 && size % 16 == 0 && relative <= bl && size <= bl - relative,
            "module range",
        )?;
        let bytes = slice(b, bo + relative, size)?;
        need(
            bytes[..4] == [0xde, 0xc0, 0x17, 0x0b]
                && u32le(bytes, 4)? == 0
                && u32le(bytes, 8)? == 20
                && u32le(bytes, 16)? == 0xffff_ffff,
            "LLVM wrapper fields",
        )?;
        let bitlen = u32le(bytes, 12)?;
        need(
            bitlen >= 4 && bitlen <= size - 20 && bytes[20..24] == [b'B', b'C', 0xc0, 0xde],
            "LLVM bitcode bounds/signature",
        )?;
        need(
            bytes[20 + bitlen..].iter().all(|v| *v == 0),
            "wrapper padding",
        )?;
        need(get(b"HASH")? == digest(bytes), "embedded module hash")?;
        modules.push(Module {
            name,
            stage: stage[0],
            offset: bo + relative,
            bytes,
        });
        pos += len;
    }
    need(
        pos + 4 == po && slice(b, pos, 4)? == b"ENDT",
        "function section trailer/count",
    )?;
    let mut ranges: Vec<_> = modules.iter().map(|m| (m.offset, m.bytes.len())).collect();
    ranges.sort_unstable();
    let mut cursor = bo;
    for (o, n) in ranges {
        need(o == cursor, "overlap or gap in modules")?;
        cursor += n;
    }
    need(cursor == b.len(), "unclaimed bitcode region")?;
    Ok(modules)
}

fn refs(s: &str, sigil: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut quote = false;
    while i < b.len() {
        if b[i] == b'"' {
            quote = !quote;
        }
        if !quote && b[i] == sigil {
            let start = i;
            i += 1;
            while b
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || b"_.$-".contains(c))
            {
                i += 1;
            }
            if i > start + 1 {
                out.push(&s[start..i]);
            }
            continue;
        }
        i += 1;
    }
    out
}
fn node<'a>(ir: &'a str, id: &str) -> Result<&'a str> {
    let prefix = format!("{id} = ");
    let rows: Vec<_> = ir.lines().filter_map(|l| l.strip_prefix(&prefix)).collect();
    need(rows.len() == 1, "missing/duplicate metadata node")?;
    Ok(rows[0])
}
fn body<'a>(ir: &'a str, entry: &str) -> Result<&'a str> {
    let (plain, quoted) = (format!("@{entry}("), format!("@\"{entry}\"("));
    let mut result = None;
    let mut offset = 0;
    for l in ir.split_inclusive('\n') {
        if l.starts_with("define ") && (l.contains(&plain) || l.contains(&quoted)) {
            need(result.is_none(), "duplicate entry definition")?;
            let tail = &ir[offset..];
            result = Some(&tail[..tail.find("\n}").ok_or("unterminated definition")? + 2]);
        }
        offset += l.len();
    }
    result.ok_or_else(|| "missing entry definition".into())
}
fn stage<'a>(ir: &'a str, entry: &str, root: &str) -> Result<Vec<&'a str>> {
    let (plain, quoted) = (format!("@{entry},"), format!("@\"{entry}\","));
    let entries = refs(node(ir, root)?, b'!')
        .into_iter()
        .map(|id| node(ir, id))
        .collect::<Result<Vec<_>>>()?;
    let matches: Vec<_> = entries
        .into_iter()
        .filter(|v| v.contains(&plain) || v.contains(&quoted))
        .collect();
    need(matches.len() == 1, "unique stage binding")?;
    let mut todo = refs(matches[0], b'!');
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        need(seen.len() <= 128, "metadata graph bound")?;
        let n = node(ir, id)?;
        todo.extend(refs(n, b'!'));
        out.push(n);
    }
    Ok(out)
}
fn dependencies<'a>(b: &'a str, roots: Vec<&'a str>) -> Result<Vec<&'a str>> {
    let mut assignments = BTreeMap::new();
    for line in b.lines() {
        if let Some((lhs, rhs)) = line.trim().split_once(" = ") {
            if lhs.starts_with('%') {
                need(assignments.insert(lhs, rhs).is_none(), "duplicate SSA")?;
            }
        }
    }
    let mut todo = roots;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(value) = todo.pop() {
        if !seen.insert(value) {
            continue;
        }
        need(seen.len() <= 512, "SSA graph bound")?;
        if let Some(rhs) = assignments.get(value) {
            out.push(*rhs);
            todo.extend(refs(rhs, b'%'));
        }
    }
    Ok(out)
}

pub fn proof(text: &str, entry: &str, kind: u8) -> Result<String> {
    need(text.len() <= 1_048_576 && kind <= 2, "AIR text bound/stage")?;
    // Narrow compiler-text gate. Unsupported spellings stop before GPU work.
    let ir = text
        .lines()
        .map(|l| l.split(';').next().unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    let b = body(&ir, entry)?;
    let metadata = stage(
        &ir,
        entry,
        ["!air.vertex", "!air.fragment", "!air.kernel"][kind as usize],
    )?;
    need(
        !metadata
            .iter()
            .any(|l| l.contains("!\"air.vertex_output\"") || l.contains("!\"air.fragment_input\"")),
        "unexpected user varying: this probe has built-in position only",
    )?;
    let mut report = format!("EXTRACTED_AIR_STAGE_PASS\nentry={entry}\nstage={kind}\n");
    match entry {
        "triangle" => {
            need(
                kind == 0
                    && metadata
                        .iter()
                        .any(|l| l.contains("!\"air.position\"") && l.contains("!\"float4\"")),
                "vertex position type",
            )?;
        }
        "emit_f32" | "emit_f16" => {
            need(kind == 1, "fragment stage")?;
            let width = if entry == "emit_f32" {
                "float4"
            } else {
                "half4"
            };
            let vector = if entry == "emit_f32" {
                "<4 x float>"
            } else {
                "<4 x half>"
            };
            let signature = b
                .lines()
                .next()
                .ok_or("definition signature")?
                .split('@')
                .next()
                .ok_or("definition return type")?;
            need(
                signature.contains(vector)
                    && b.lines()
                        .filter(|l| l.trim_start().starts_with("ret "))
                        .all(|l| l.contains(vector)),
                "definition and return instruction color width",
            )?;
            let outputs: Vec<_> = metadata
                .iter()
                .filter(|l| l.contains("!\"air.render_target\""))
                .collect();
            need(
                outputs.len() == 1
                    && outputs[0].contains("!\"air.render_target\", i32 0, i32 0")
                    && outputs[0].contains(&format!("!\"{width}\"")),
                "exact color-zero output type",
            )?;
            let returns: Vec<_> = b
                .lines()
                .filter(|l| l.trim_start().starts_with("ret "))
                .flat_map(|l| refs(l, b'%'))
                .collect();
            need(!returns.is_empty(), "live returned SSA required")?;
            let deps = dependencies(b, returns)?;
            need(
                deps.iter()
                    .any(|l| l.starts_with("load ") && l.contains("addrspace(1)")),
                "returned value must depend on device input",
            )?;
            need(
                b.lines()
                    .any(|l| l.trim_start().starts_with("store ") && l.contains("addrspace(1)")),
                "device trace store",
            )?;
            need(
                !b.contains("air.read_texture")
                    && !b.contains("air.sample_texture")
                    && !b.contains("fmul ")
                    && !b.contains("fadd ")
                    && !b.contains("fsub ")
                    && !b.contains("air.fma"),
                "no shader-side blend or destination access",
            )?;
            if entry == "emit_f16" {
                need(
                    deps.iter().any(|l| {
                        l.starts_with("fptrunc <4 x float>")
                            || (l.contains("call ")
                                && l.contains("@air.convert.f.")
                                && l.contains("f16")
                                && l.contains("f32"))
                    }),
                    "live f32-to-f16 output conversion",
                )?;
            } else {
                need(
                    !b.contains("fptrunc") && !b.contains("<4 x half>"),
                    "float output must not narrow",
                )?;
            }
            report += &format!("return_type={width}\n");
        }
        "observe" => {
            need(kind == 2, "readback kernel stage")?;
            let roots: Vec<_> = b
                .lines()
                .filter(|l| l.trim_start().starts_with("store ") && l.contains("addrspace(1)"))
                .flat_map(|l| refs(l, b'%'))
                .collect();
            let deps = dependencies(b, roots)?;
            for ty in ["v4f32", "v4f16"] {
                need(
                    deps.iter().any(|l| {
                        l.contains("call ") && l.contains(&format!("@air.read_texture_2d.{ty}("))
                    }),
                    "both typed texture reads must reach device stores",
                )?;
            }
            need(
                !b.contains("air.sample_texture") && !b.contains("air.write_texture"),
                "read-only unsampled observation",
            )?;
        }
        _ => return Err("unexpected probe entry".into()),
    }
    for line in metadata {
        report += line;
        report.push('\n');
    }
    report += "COMPILER_MATH_METADATA (absence is not preservation proof):\n";
    for line in ir.lines().filter(|l| {
        l.contains("air.compile")
            || l.contains("denormal")
            || l.contains("unsafe-fp")
            || l.contains("no-nans-fp")
    }) {
        report += line;
        report.push('\n');
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha_known_answers() {
        assert_eq!(
            sha(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    fn container() -> Vec<u8> {
        fn tag(r: &mut Vec<u8>, n: &[u8], v: &[u8]) {
            r.extend(n);
            r.extend((v.len() as u16).to_le_bytes());
            r.extend(v);
        }
        let mut air = Vec::new();
        for v in [0x0b17_c0deu32, 0, 20, 4, 0xffff_ffff] {
            air.extend(v.to_le_bytes());
        }
        air.extend([b'B', b'C', 0xc0, 0xde]);
        air.resize(32, 0);
        let mut record = vec![0; 4];
        tag(&mut record, b"NAME", b"triangle\0");
        tag(&mut record, b"TYPE", &[0]);
        tag(&mut record, b"HASH", &digest(&air));
        tag(&mut record, b"MDSZ", &32u64.to_le_bytes());
        tag(&mut record, b"OFFT", &[0; 24]);
        tag(&mut record, b"VERS", &[0; 8]);
        record.extend(b"ENDT");
        let n = record.len();
        record[..4].copy_from_slice(&(n as u32).to_le_bytes());
        let po = 88 + n + 8;
        let qo = po + 8;
        let bo = qo + 8;
        let mut b = vec![0; 88];
        b[..4].copy_from_slice(b"MTLB");
        b[6] = 2;
        for (p, v) in [
            (16, bo + 32),
            (24, 88),
            (32, n),
            (40, po),
            (48, 8),
            (56, qo),
            (64, 8),
            (72, bo),
            (80, 32),
        ] {
            b[p..p + 8].copy_from_slice(&(v as u64).to_le_bytes());
        }
        b.extend(1u32.to_le_bytes());
        b.extend(record);
        b.extend(b"ENDT");
        for _ in 0..2 {
            b.extend(4u32.to_le_bytes());
            b.extend(b"ENDT");
        }
        b.extend(air);
        b
    }
    #[test]
    fn container_bounds_hash_and_no_magic_scan() {
        let b = container();
        let m = extract(&b).unwrap();
        assert_eq!(m[0].name, "triangle");
        assert_eq!(m[0].bytes.len(), 32);
        for n in 0..b.len() {
            assert!(extract(&b[..n]).is_err());
        }
        for p in [
            0,
            6,
            16,
            24,
            32,
            40,
            56,
            72,
            80,
            88,
            92,
            b.len() - 9,
            b.len() - 1,
        ] {
            let mut bad = b.clone();
            bad[p] ^= 1;
            assert!(extract(&bad).is_err(), "offset {p}");
        }
        let mut bad = b.clone();
        bad.extend([b'B', b'C', 0xc0, 0xde]);
        assert!(extract(&bad).is_err());
    }
    fn fragment() -> String {
        [
            "define <4 x float> @emit_f32(ptr addrspace(1) %input, ptr addrspace(1) %trace) {",
            "%a = load <4 x i32>, ptr addrspace(1) %input",
            "%v = bitcast <4 x i32> %a to <4 x float>",
            "store <4 x i32> %a, ptr addrspace(1) %trace",
            "ret <4 x float> %v",
            "}",
            "!air.fragment = !{!0}",
            "!0 = !{ptr @emit_f32, !1}",
            "!1 = !{!2, !3}",
            "!2 = !{!\"air.render_target\", i32 0, i32 0, !\"air.arg_type_name\", !\"float4\"}",
            "!3 = !{i32 0, !\"air.position\", !\"air.arg_type_name\", !\"float4\"}",
        ]
        .join("\n")
    }
    #[test]
    fn fragment_live_route_and_negatives() {
        let good = fragment();
        assert!(proof(&good, "emit_f32", 1).is_ok());
        for bad in [
            good.replace("load <4 x i32>", "freeze <4 x i32>"),
            good.replace("ret <4 x float> %v", "ret <4 x float> zeroinitializer"),
            good.replace("store <4 x i32>", "; store <4 x i32>"),
            good.replace("define <4 x float>", "define <4 x half>"),
            good.replace("ret <4 x float>", "ret <4 x i32>"),
            good.replace(
                "!\"air.render_target\", i32 0",
                "!\"air.render_target\", i32 1",
            ),
            good.replace("!air.fragment = !{!0}", "!air.fragment = !{!1}"),
            good.replace("%v = bitcast", "%v = fptrunc"),
            good.replace("!1 = !{!2, !3}", "!1 = !{!2, !3, !4}")
                + "\n!4 = !{!\"air.fragment_input\", !\"generated(1xDv4_f)\"}",
        ] {
            assert!(proof(&bad, "emit_f32", 1).is_err());
        }
        assert!(proof(&good, "emit_f32", 0).is_err());
        assert!(proof("declare <4 x float> @emit_f32()", "emit_f32", 1).is_err());
    }
    #[test]
    fn half_output_conversion_is_live() {
        let good = fragment()
            .replace("emit_f32", "emit_f16")
            .replace("define <4 x float>", "define <4 x half>")
            .replace(
                "ret <4 x float> %v",
                "%h = fptrunc <4 x float> %v to <4 x half>\nret <4 x half> %h",
            )
            .replacen("!\"float4\"", "!\"half4\"", 1);
        assert!(proof(&good, "emit_f16", 1).is_ok());
        assert!(proof(
            &good.replace("ret <4 x half> %h", "ret <4 x half> zeroinitializer"),
            "emit_f16",
            1
        )
        .is_err());
    }
    #[test]
    fn both_read_types_must_reach_a_store() {
        let ir = [
            "define void @observe(ptr addrspace(1) %out) {",
            "%f = call <4 x float> @air.read_texture_2d.v4f32()",
            "%h = call <4 x half> @air.read_texture_2d.v4f16()",
            "store <4 x float> %f, ptr addrspace(1) %out",
            "store <4 x half> %h, ptr addrspace(1) %out",
            "ret void",
            "}",
            "!air.kernel = !{!0}",
            "!0 = !{ptr @observe, !1}",
            "!1 = !{}",
        ]
        .join("\n");
        assert!(proof(&ir, "observe", 2).is_ok());
        assert!(proof(
            &ir.replace("store <4 x half> %h", "store <4 x half> zeroinitializer"),
            "observe",
            2
        )
        .is_err());
        assert!(proof(
            &ir.replace("@air.read_texture_2d.v4f16", "@air.sample_texture_2d.v4f16"),
            "observe",
            2
        )
        .is_err());
    }
}
