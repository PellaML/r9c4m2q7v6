// Synthetic public Metal inputs only. This generates descriptors, not GPU answers.
#![forbid(unsafe_code)]

use std::{env, fs, io, io::Write, path::Path};

const MAX_CASES: usize = 48;
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const FULL: [u32; 4] = [0, 0, 8, 8];
const LEFT: [u32; 4] = [0, 0, 4, 8];
const RIGHT: [u32; 4] = [4, 0, 4, 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Face {
    compare: &'static str,
    fail: &'static str,
    depth_fail: &'static str,
    pass: &'static str,
    read_mask: u32,
    write_mask: u32,
}

impl Face {
    fn new(compare: &'static str, pass: &'static str) -> Self {
        Self {
            compare,
            fail: "keep",
            depth_fail: "keep",
            pass,
            read_mask: u32::MAX,
            write_mask: u32::MAX,
        }
    }
    fn json(self) -> String {
        format!(
            concat!(
                "{{\"compare\":\"{}\",\"fail\":\"{}\",\"depthFail\":\"{}\",",
                "\"pass\":\"{}\",\"readMask\":{},\"writeMask\":{}}}"
            ),
            self.compare, self.fail, self.depth_fail, self.pass, self.read_mask, self.write_mask
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Draw {
    mode: &'static str,
    front: Face,
    back: Face,
    front_ref: u32,
    back_ref: u32,
    split_ref: bool,
    rect: [u32; 4],
    reverse: bool,
    winding: &'static str,
    color: [u8; 4],
    color_write: bool,
    discard: &'static str,
}

impl Draw {
    fn new(compare: &'static str, pass: &'static str, reference: u32) -> Self {
        Self {
            mode: "enabled",
            front: Face::new(compare, pass),
            back: Face::new(compare, pass),
            front_ref: reference,
            back_ref: reference,
            split_ref: false,
            rect: FULL,
            reverse: false,
            winding: "ccw",
            color: RED,
            color_write: true,
            discard: "none",
        }
    }
    fn faces(mut self, f: impl FnOnce(&mut Face)) -> Self {
        f(&mut self.front);
        self.back = self.front;
        self
    }
    fn hidden(mut self) -> Self {
        self.color_write = false;
        self
    }
    fn rect(mut self, r: [u32; 4]) -> Self {
        self.rect = r;
        self
    }
    fn color(mut self, c: [u8; 4]) -> Self {
        self.color = c;
        self
    }
    fn json(&self) -> String {
        format!(
            concat!(
                "{{\"mode\":\"{}\",\"front\":{},\"back\":{},",
                "\"frontRef\":{},\"backRef\":{},\"splitRef\":{},\"rect\":{:?},",
                "\"reverse\":{},\"winding\":\"{}\",\"color\":{:?},",
                "\"colorWrite\":{},\"discard\":\"{}\"}}"
            ),
            self.mode,
            self.front.json(),
            self.back.json(),
            self.front_ref,
            self.back_ref,
            self.split_ref,
            self.rect,
            self.reverse,
            self.winding,
            self.color,
            self.color_write,
            self.discard
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Case {
    name: &'static str,
    topic: &'static str,
    load: &'static str,
    store: &'static str,
    clear: u32,
    draws: Vec<Draw>,
    anchor_color: Option<[u8; 4]>,
    anchor_stencil: Option<u8>,
}

impl Case {
    fn new(name: &'static str, topic: &'static str, clear: u32, draws: Vec<Draw>) -> Self {
        Self {
            name,
            topic,
            load: "clear",
            store: "store",
            clear,
            draws,
            anchor_color: None,
            anchor_stencil: None,
        }
    }
    fn anchor(mut self, color: [u8; 4], stencil: u8) -> Self {
        self.anchor_color = Some(color);
        self.anchor_stencil = Some(stencil);
        self
    }
    fn json(&self) -> String {
        let color = self
            .anchor_color
            .map_or("null".to_owned(), |v| format!("{v:?}"));
        let stencil = self
            .anchor_stencil
            .map_or("null".to_owned(), |v| v.to_string());
        format!(
            concat!(
                "{{\"name\":\"{}\",\"topic\":\"{}\",\"load\":\"{}\",",
                "\"store\":\"{}\",\"clear\":{},\"draws\":[{}],",
                "\"anchorColor\":{},\"anchorStencil\":{}}}"
            ),
            self.name,
            self.topic,
            self.load,
            self.store,
            self.clear,
            self.draws
                .iter()
                .map(Draw::json)
                .collect::<Vec<_>>()
                .join(","),
            color,
            stencil
        )
    }
}

fn cases() -> Vec<Case> {
    let always = |op, r| Draw::new("always", op, r);
    let ne = |op, r| Draw::new("notEqual", op, r);
    let mut v = vec![
        Case::new("clear-0", "clear-anchor", 0, vec![]).anchor(BLACK, 0),
        Case::new("clear-1", "clear-anchor", 1, vec![]).anchor(BLACK, 1),
        Case::new("clear-255", "clear-anchor", 255, vec![]).anchor(BLACK, 255),
        Case::new("clear-256", "clear-u32-narrowing-observation", 256, vec![]),
        Case::new(
            "clear-max",
            "clear-u32-narrowing-observation",
            u32::MAX,
            vec![],
        ),
        Case::new("always-keep", "keep", 1, vec![always("keep", 0)]).anchor(RED, 1),
        Case::new("always-zero", "zero", 1, vec![always("zero", 0)]).anchor(RED, 0),
        Case::new(
            "always-replace-1",
            "replace-anchor-and-negative-reference",
            0,
            vec![always("replace", 1)],
        )
        .anchor(RED, 1),
        Case::new("always-replace-0", "replace", 1, vec![always("replace", 0)]).anchor(RED, 0),
        Case::new("ne-equal-keep", "stencil-fail-keep", 1, vec![ne("keep", 1)]).anchor(BLACK, 1),
        Case::new(
            "ne-different-keep",
            "stencil-pass-keep",
            0,
            vec![ne("keep", 1)],
        )
        .anchor(RED, 0),
        Case::new("ne-pass-zero", "stencil-pass-zero", 1, vec![ne("zero", 0)]).anchor(RED, 0),
        Case::new(
            "ne-pass-replace",
            "stencil-pass-replace",
            0,
            vec![ne("replace", 1)],
        )
        .anchor(RED, 1),
        Case::new(
            "ne-fail-zero",
            "stencil-fail-zero",
            1,
            vec![ne("keep", 1).faces(|f| f.fail = "zero")],
        )
        .anchor(BLACK, 0),
        Case::new(
            "ne-fail-replace",
            "stencil-fail-replace-read-mask-zero",
            0,
            vec![ne("keep", 1).faces(|f| {
                f.read_mask = 0;
                f.fail = "replace";
            })],
        ),
        Case::new(
            "write-mask-zero",
            "no-stencil-write",
            1,
            vec![always("zero", 0).faces(|f| f.write_mask = 0)],
        )
        .anchor(RED, 1),
        Case::new(
            "write-mask-low",
            "masked-replace",
            0xa5,
            vec![always("replace", 0x3c).faces(|f| f.write_mask = 0x0f)],
        ),
        Case::new(
            "write-mask-high-byte",
            "masked-zero",
            0xa5,
            vec![always("zero", 0).faces(|f| f.write_mask = 0xf0)],
        ),
        Case::new(
            "write-mask-high-u32",
            "mask-u32-narrowing-observation",
            0xa5,
            vec![always("replace", u32::MAX).faces(|f| f.write_mask = 0xffff_ff00)],
        ),
        Case::new(
            "write-mask-mixed-u32",
            "mask-u32-narrowing-observation",
            0xa4,
            vec![always("replace", u32::MAX).faces(|f| f.write_mask = 0x8000_0001)],
        ),
        Case::new(
            "read-mask-low-equal",
            "masked-compare",
            0xa5,
            vec![ne("keep", 0x35).faces(|f| f.read_mask = 0x0f)],
        ),
        Case::new(
            "read-mask-low-different",
            "masked-compare",
            0xa5,
            vec![ne("keep", 0x34).faces(|f| f.read_mask = 0x0f)],
        ),
        Case::new(
            "reference-256",
            "reference-u32-narrowing-observation",
            0,
            vec![ne("keep", 256)],
        ),
        Case::new(
            "reference-max",
            "reference-u32-narrowing-observation",
            255,
            vec![ne("keep", u32::MAX)],
        ),
        Case::new(
            "reference-high-plus-one",
            "reference-u32-narrowing-observation",
            1,
            vec![ne("keep", 0x8000_0001)],
        ),
        Case::new(
            "read-mask-high-u32",
            "mask-and-reference-u32-observation",
            0,
            vec![ne("keep", u32::MAX).faces(|f| f.read_mask = 0xffff_ff00)],
        ),
        Case::new(
            "replace-256",
            "replace-u32-narrowing-observation",
            1,
            vec![always("replace", 256)],
        ),
        Case::new(
            "replace-257",
            "replace-u32-narrowing-observation",
            0,
            vec![always("replace", 257)],
        ),
        Case::new(
            "replace-max",
            "replace-u32-narrowing-observation",
            0,
            vec![always("replace", u32::MAX)],
        ),
    ];

    // Bind a real default state object; nil is not a valid encoder state.
    // Keep the default-state anchor and exercise reference independence at zero.
    for (name, reference) in [("off-default", u32::MAX), ("off-default-ref-zero", 0)] {
        let mut d = always("replace", reference).color(GREEN);
        d.mode = "default";
        v.push(Case::new(name, "stencil-off", 1, vec![d]).anchor(GREEN, 1));
    }
    let mut off = always("zero", 0).color(GREEN);
    off.mode = "default";
    v.push(
        Case::new(
            "enabled-to-default-off",
            "state-transition",
            0,
            vec![always("replace", 1).hidden(), off],
        )
        .anchor(GREEN, 1),
    );

    let write_left = always("replace", 1).rect(LEFT).hidden();
    let show_other = ne("keep", 1).color(BLUE);
    v.push(Case::new(
        "overlap-write-then-test",
        "draw-order-and-color-mask-zero",
        0,
        vec![write_left.clone(), show_other.clone()],
    ));
    v.push(Case::new(
        "overlap-test-then-write",
        "reversed-draw-order",
        0,
        vec![show_other.clone(), write_left.clone()],
    ));

    // Keep the same shader entry point and uniforms except discard mode. No
    // early-tests attribute, shader depth output, or shader stencil reference.
    for (name, mode) in [
        ("discard-none", "none"),
        ("discard-all", "all"),
        ("discard-left", "left"),
    ] {
        let mut d = always("replace", 1);
        d.discard = mode;
        v.push(Case::new(
            name,
            "discard-versus-reference-observation",
            0,
            vec![d],
        ));
    }
    let mut fail_discard = ne("keep", 1).faces(|f| f.fail = "zero");
    fail_discard.discard = "all";
    v.push(Case::new(
        "discard-stencil-fail",
        "discard-and-fail-operation-observation",
        1,
        vec![fail_discard],
    ));

    let a = always("replace", 1).rect(LEFT);
    let mut b = always("replace", 1).rect(RIGHT).color(GREEN);
    b.reverse = true;
    v.push(Case::new(
        "opposed-identical-faces",
        "winding-identical-descriptors",
        0,
        vec![a, b],
    ));
    for (name, winding) in [("opposed-split-ccw", "ccw"), ("opposed-split-cw", "cw")] {
        let mut a = always("replace", 1).rect(LEFT);
        a.split_ref = true;
        a.back_ref = 7;
        a.winding = winding;
        // Front replaces, back zeros: the resulting pair diagnoses face choice.
        a.back.pass = "zero";
        let mut b = a.clone();
        b.rect = RIGHT;
        b.reverse = true;
        b.color = GREEN;
        v.push(Case::new(
            name,
            "opposed-face-state-and-split-reference-observation",
            3,
            vec![a, b],
        ));
    }
    let mut split = always("replace", 1).rect(LEFT);
    split.split_ref = true;
    split.back_ref = 7;
    let mut split_reverse = split.clone();
    split_reverse.rect = RIGHT;
    split_reverse.reverse = true;
    v.push(Case::new(
        "opposed-split-refs",
        "independent-front-back-reference-observation",
        0,
        vec![split, split_reverse],
    ));

    // DontCare load is never read before a full-surface, full-mask overwrite.
    let init = always("replace", 0).hidden();
    let mut dc = Case::new(
        "dontcare-load-initialized",
        "defined-pass-local-dontcare-load",
        0,
        vec![init.clone(), write_left.clone(), show_other.clone()],
    );
    dc.load = "dontCare";
    v.push(dc);
    let mut dc = Case::new(
        "dontcare-store",
        "color-only-no-post-pass-stencil-publication",
        0,
        vec![write_left.clone(), show_other.clone()],
    );
    dc.store = "dontCare";
    v.push(dc);
    let mut dc = Case::new(
        "dontcare-load-store",
        "initialized-color-only-no-stencil-publication",
        0,
        vec![init, write_left, show_other],
    );
    dc.load = "dontCare";
    dc.store = "dontCare";
    v.push(dc);
    assert!(v.len() <= MAX_CASES);
    v
}

fn vertices() -> Vec<u8> {
    let a = [-1.0f32, -1.0, 0.0, 1.0];
    let b = [3.0f32, -1.0, 0.0, 1.0];
    let c = [-1.0f32, 3.0, 0.0, 1.0];
    [a, b, c, a, c, b]
        .into_iter()
        .flatten()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect()
}

fn manifest() -> String {
    let all = cases();
    format!(concat!("{{\n\"schema\":1,\"caseCount\":{},\"width\":8,\"height\":8,",
                    "\"vertexFile\":\"vertices.bin\",\"vertexLayout\":\"6 little-endian binary32x4 positions; normal then reversed triangle\",",
                    "\"colorFormat\":\"rgba8Uint\",\"stencilFormat\":\"stencil8\",",
                    "\"policy\":\"synthetic descriptors; anchors are validation expectations, never native evidence; other behavior is observation-only\",",
                    "\"cases\":[\n{}\n]}}\n"),
            all.len(), all.iter().map(Case::json).collect::<Vec<_>>().join(",\n"))
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(data)
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected fresh output directory",
        ));
    }
    let out = Path::new(&args[1]);
    // Refuse any partial overwrite before opening either output.
    if out.join("cases.json").exists() || out.join("vertices.bin").exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "generated input already exists",
        ));
    }
    write_new(&out.join("cases.json"), manifest().as_bytes())?;
    write_new(&out.join("vertices.bin"), &vertices())?;
    println!(
        "synthetic_cases={} max_cases={} gpu_answers_generated=0",
        cases().len(),
        MAX_CASES
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_deterministic_and_unique() {
        let all = cases();
        assert_eq!(all.len(), 45);
        assert_eq!(all, cases());
        assert_eq!(manifest(), manifest());
        assert!(manifest().len() < 131072);
        let names = all
            .iter()
            .map(|c| c.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(all.len(), names.len());
        for c in &all {
            assert!(c.draws.len() <= 3);
            for d in &c.draws {
                assert!(d.rect[0] + d.rect[2] <= 8 && d.rect[1] + d.rect[3] <= 8);
                assert!(!d.color.contains(&0xcd));
                if !d.split_ref {
                    assert_eq!(d.front_ref, d.back_ref);
                }
            }
        }
    }
    #[test]
    fn exact_vertex_words_and_opposed_faces() {
        let data = vertices();
        assert_eq!(data.len(), 96);
        assert_eq!(&data[..16], &data[48..64]);
        assert_eq!(&data[16..32], &data[80..96]);
        assert_eq!(&data[32..48], &data[64..80]);
        assert_eq!(
            u32::from_le_bytes(data[..4].try_into().unwrap()),
            (-1.0f32).to_bits()
        );
    }
    #[test]
    fn dontcare_has_no_uninitialized_input_or_stencil_assertion() {
        for c in cases() {
            if c.load == "dontCare" {
                let d = &c.draws[0];
                assert_eq!(d.front, Face::new("always", "replace"));
                assert_eq!(d.front, d.back);
                assert_eq!(d.rect, FULL);
                assert_eq!(d.discard, "none");
                assert!(!d.color_write);
            }
            if c.store == "dontCare" {
                assert_eq!(c.anchor_stencil, None);
            }
        }
    }
    #[test]
    fn default_state_controls_have_real_state_modes_and_exact_anchors() {
        let all = cases();
        assert!(!all.iter().any(|c| c.name == "off-nil"));
        for (name, reference) in [("off-default", u32::MAX), ("off-default-ref-zero", 0)] {
            let c = all.iter().find(|c| c.name == name).unwrap();
            assert_eq!(c.anchor_color, Some(GREEN));
            assert_eq!(c.anchor_stencil, Some(1));
            assert_eq!(c.draws.len(), 1);
            assert_eq!(c.draws[0].mode, "default");
            assert_eq!(c.draws[0].front_ref, reference);
            assert_eq!(c.draws[0].back_ref, reference);
        }
        for d in all.iter().flat_map(|c| &c.draws) {
            assert!(matches!(d.mode, "enabled" | "default"));
        }
    }

    #[test]
    fn full_u32_values_are_not_narrowed_by_generator() {
        let all = cases();
        let j = manifest();
        for n in [u32::MAX, 256, 257, 0xffff_ff00, 0x8000_0001] {
            assert!(j.contains(&n.to_string()));
        }
        assert!(all.iter().any(|c| c.clear == u32::MAX));
        assert!(all
            .iter()
            .flat_map(|c| &c.draws)
            .any(|d| d.front.read_mask == 0xffff_ff00));
        assert!(all
            .iter()
            .flat_map(|c| &c.draws)
            .any(|d| d.front.write_mask == 0xffff_ff00));
    }
}
