//! Independent exact model for owned odd-framebuffer inputs, not a native renderer.
#![forbid(unsafe_code)]
use std::{env, fs, io, path::Path};
const W: usize = 35;
const H: usize = 27;
const N: usize = W * H;
const HEADER: &str = "id\tname\tderivatives\tx\ty\twidth\theight\n";
#[derive(Clone, Debug)]
struct Case { id: usize, name: String, derivative: bool, x: usize, y: usize }
fn cases() -> Vec<Case> {
    [("interior",3,5),("right",25,5),("bottom",3,19),("corner",25,19)]
        .into_iter().enumerate().flat_map(|(region,(name,x,y))| {
            [false,true].map(|derivative| Case { id: region*2+usize::from(derivative),
                name: format!("{name}-{}",if derivative {"derivative"} else {"scalar"}),
                derivative,x,y })
        }).collect()
}
fn tsv() -> String {
    let mut out=String::from(HEADER);
    for c in cases() { out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t10\t8\n",c.id,c.name,u8::from(c.derivative),c.x,c.y)); }
    out
}
// This deliberately accepts only exactly representable finite normal/zero half
// values. It is not a rounding approximation or a general floating-point model.
fn half_exact(v: f32) -> u16 {
    let b=v.to_bits();
    if b&0x7fff_ffff==0 { return (b>>16) as u16; }
    let exponent=((b>>23)&255) as i32-127+15;
    assert!((1..31).contains(&exponent) && b&0x1fff==0,"non-exact half {v:?}");
    ((b>>16)&0x8000) as u16 | ((exponent as u16)<<10) | ((b>>13)&1023) as u16
}
fn bytes(words: &[u16]) -> Vec<u8> { words.iter().flat_map(|w|w.to_le_bytes()).collect() }
fn words(data: &[u8]) -> io::Result<Vec<u16>> {
    if data.len()%2!=0 { return Err(error("odd half byte length")); }
    Ok(data.chunks_exact(2).map(|b|u16::from_le_bytes(b.try_into().unwrap())).collect())
}
fn color_seed() -> Vec<u16> { [0.125,0.25,0.5,0.75].map(half_exact).repeat(N) }
fn fetch_at(x:usize,y:usize) -> [f32;2] {
    [0.25+((3*x+5*y)&3) as f32/16.0,0.5+((x+2*y)&3) as f32/16.0]
}
fn fetch_seed() -> Vec<u16> {
    (0..H).flat_map(|y|(0..W).flat_map(move|x|fetch_at(x,y).map(half_exact))).collect()
}
fn varying(x:usize,y:usize) -> f32 { (x as f32+0.5)/16.0+(y as f32+0.5)/32.0 }
#[derive(Clone, Copy)]
enum Counter { None, ClampHelperGeometry, ZeroFetch }
#[derive(Debug, PartialEq)]
struct Model { color:Vec<u16>, fetch:Vec<u16>, stencil:Vec<u8> }
fn model(c:&Case,counter:Counter) -> Model {
    let mut m=Model{color:color_seed(),fetch:fetch_seed(),stencil:vec![43;N]};
    for y in c.y..c.y+8 { for x in c.x..c.x+10 {
        let operand=|xx:usize,yy:usize| {
            if matches!(counter,Counter::ClampHelperGeometry) { varying(xx.min(W-1),yy.min(H-1)) }
            else { varying(xx,yy) }
        };
        let (dx,dy)=if c.derivative {
            (operand((x&!1)+1,y)-operand(x&!1,y),operand(x,(y&!1)+1)-operand(x,y&!1))
        } else { (0.0625,0.03125) };
        let fw=dx.abs()+dy.abs();
        // Fetch only the covered in-bounds pixel. No OOB fetch value is modeled.
        let prior=if matches!(counter,Counter::ZeroFetch) {[0.0;2]} else {fetch_at(x,y)};
        let out=[prior[0]+dx,prior[1]+dy,fw,0.25*varying(x,y)+fw].map(half_exact);
        m.color[(y*W+x)*4..(y*W+x)*4+4].copy_from_slice(&out);
    }}
    m
}
fn difference<T:PartialEq>(a:&[T],b:&[T]) -> usize {
    a.iter().zip(b).filter(|(a,b)|a!=b).count()+a.len().abs_diff(b.len())
}
fn error(s:impl Into<String>) -> io::Error { io::Error::other(s.into()) }
fn plane(raw:&[u8],at:&mut usize,bpp:usize) -> io::Result<Vec<u8>> {
    let pitch=(W*bpp).div_ceil(256)*256;
    let length=512+pitch*H;
    let p=raw.get(*at..*at+length).ok_or_else(||error("short native plane"))?;
    *at+=length;
    for (i,&v) in p.iter().enumerate() {
        let payload=i>=256 && i<256+pitch*H && (i-256)%pitch<W*bpp;
        if !payload && v!=0xcd { return Err(error("native readback guard/padding changed")); }
    }
    Ok((0..H).flat_map(|y|p[256+y*pitch..256+y*pitch+W*bpp].iter().copied()).collect())
}
fn query(data:&[u8]) -> io::Result<Vec<u8>> {
    if data.len()!=N*4 { return Err(error("stencil query size")); }
    data.chunks_exact(4).map(|b| {
        if b[1]==255-b[0] && b[2]==83 && b[3]==255 {Ok(b[0])} else {Err(error("stencil query signature"))}
    }).collect()
}
fn observed(root:&Path,c:&Case) -> io::Result<Model> {
    let raw=fs::read(root.join(format!("case{:02}.bin",c.id)))?;
    if raw.len()!=58368 { return Err(error("native case length")); }
    let mut at=0;
    if words(&plane(&raw,&mut at,8)?)?!=color_seed() ||
       words(&plane(&raw,&mut at,4)?)?!=fetch_seed() ||
       query(&plane(&raw,&mut at,4)?)?!=vec![43;N] { return Err(error("native initial pixels differ")); }
    let color=words(&plane(&raw,&mut at,8)?)?;
    let fetch=words(&plane(&raw,&mut at,4)?)?;
    let stencil=query(&plane(&raw,&mut at,4)?)?;
    if at!=raw.len() { return Err(error("native case tail")); }
    Ok(Model{color,fetch,stencil})
}
fn prepare(root:&Path) -> io::Result<()> {
    if root.exists() { return Err(error("input destination must be new")); }
    fs::create_dir_all(root)?;
    fs::write(root.join("cases.tsv"),tsv())?;
    fs::write(root.join("seed-rgba16.bin"),bytes(&color_seed()))?;
    fs::write(root.join("seed-rg16.bin"),bytes(&fetch_seed()))?;
    println!("PREPARED_OWNED_INPUTS cases=8 real_width=35 real_height=27 native_executed=false");
    Ok(())
}
fn check(root:&Path,wrong:bool) -> io::Result<()> {
    if fs::read_to_string(root.join("cases.tsv"))?!=tsv() ||
       fs::read(root.join("seed-rgba16.bin"))?!=bytes(&color_seed()) ||
       fs::read(root.join("seed-rg16.bin"))?!=bytes(&fetch_seed()) { return Err(error("owned inputs changed")); }
    let cal=fs::read(root.join("query-calibration.bin"))?;
    let mut at=0;
    let q=query(&plane(&cal,&mut at,4)?)?;
    if at!=cal.len() || q!=(0..N).map(|i|(i&255) as u8).collect::<Vec<_>>() { return Err(error("all256 query calibration")); }
    let cs=cases();
    let mut got=Vec::new();
    for c in &cs { got.push(observed(root,c)?); }
    let mut rows=Vec::new();let mut mismatches=0;
    for c in &cs {
        let expected=model(c,if wrong {Counter::ClampHelperGeometry} else {Counter::None});
        let o=&got[c.id];
        let d=[difference(&o.color,&expected.color),difference(&o.fetch,&expected.fetch),difference(&o.stencil,&expected.stencil)];
        mismatches+=d.iter().sum::<usize>();
        rows.push(format!("{{\"id\":{},\"name\":\"{}\",\"derivative_variant\":{},\"covered_pixels\":80,\"color_half_word_mismatch\":{},\"RG16_half_word_mismatch\":{},\"stencil_byte_mismatch\":{}}}",c.id,c.name,c.derivative,d[0],d[1],d[2]));
    }
    let paired=(0..4).all(|i|got[i*2]==got[i*2+1]);
    let witnesses:Vec<_>=[3,5,7].into_iter().map(|id|difference(&got[id].color,&model(&cs[id],Counter::ClampHelperGeometry).color)).collect();
    let fetch_witnesses:Vec<_>=cs.iter().map(|c|difference(&got[c.id].color,&model(c,Counter::ZeroFetch).color)).collect();
    let controls=paired && witnesses.iter().all(|n|*n>0) && fetch_witnesses.iter().all(|n|*n>0);
    let pass=mismatches==0 && controls;
    let report=format!("{{\"status\":\"{}\",\"native_model_qualification\":{},\"deliberately_wrong_clamped_geometry_model\":{},\"rows\":[{}],\"scalar_derivative_pairs_equal\":{},\"right_bottom_corner_clamped_geometry_differences\":{:?},\"zero_fetch_differences_by_case\":{:?},\"exact_tolerance\":0,\"modeled_OOB_fetch_values\":false,\"invocation_counts_measured\":false,\"platform_scope\":\"actual native.json build/device/compiler, not runner filename\"}}\n",if pass {"NATIVE_MODEL_MATCH_WITH_NEGATIVE_WITNESSES"} else {"MODEL_MISMATCH_OR_NONDISCRIMINATING_CONTROL"},pass,wrong,rows.join(","),paired,witnesses,fetch_witnesses);
    fs::write(root.join(if wrong {"negative-check.json"} else {"model-check.json"}),report)?;
    if !pass { return Err(error(format!("model mismatches={mismatches}; controls={controls}; preserve observations"))); }
    println!("NATIVE_MODEL_MATCH cases=8 exact_tolerance=0");Ok(())
}
fn self_test() -> io::Result<()> {
    let cs=cases();let models:Vec<_>=cs.iter().map(|c|model(c,Counter::None)).collect();
    for i in 0..4 { assert_eq!(models[i*2],models[i*2+1]); }
    let d:Vec<_>=cs.iter().map(|c|difference(&models[c.id].color,&model(c,Counter::ClampHelperGeometry).color)).collect();
    assert_eq!(d,[0,0,0,24,0,30,0,52]);
    for c in &cs {
        assert_eq!(difference(&models[c.id].color,&color_seed()),320);
        assert_eq!(difference(&models[c.id].color,&model(c,Counter::ZeroFetch).color),160);
        assert_eq!(models[c.id].fetch,fetch_seed());assert_eq!(models[c.id].stencil,vec![43;N]);
    }
    for y in 0..=H { for x in 0..=W { half_exact(varying(x,y)); }}
    println!("HOST_MODEL_SELF_TEST_PASS native_executed=false cases=8 clamped_geometry_words={d:?} zero_fetch_words_per_case=160");
    Ok(())
}
fn main() {
    let a:Vec<_>=env::args_os().collect();
    let result=match a.get(1).and_then(|s|s.to_str()) {
        Some("--self-test") if a.len()==2=>self_test(),
        Some("prepare") if a.len()==3=>prepare(Path::new(&a[2])),
        Some("check") if a.len()==3=>check(Path::new(&a[2]),false),
        Some("check-wrong") if a.len()==3=>check(Path::new(&a[2]),true),
        _=>Err(error("usage: odd-edge-check --self-test | prepare NEW_RESULTS | check RESULTS | check-wrong RESULTS")),
    };
    if let Err(e)=result { eprintln!("{e}");std::process::exit(3); }
}
