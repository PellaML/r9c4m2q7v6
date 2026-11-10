//! Independent bounded model for owned dyadic inputs, never native output synthesis.
#![forbid(unsafe_code)]
use std::{env,fs,io,path::Path};
const W:usize=32;
const H:usize=32;
const N:usize=W*H;
const GUARD:usize=256;
const POISON:u8=0xcd;
const HEADER:&str="id\tname\tattached\tcompare\toperation\tearly\tdiscard\tdraws\tcolor_mask\tsplit\tchecker\treference\tread_mask\twrite_mask\tnegative\n";
#[derive(Clone,Debug)]
struct Case { id:usize,name:&'static str,attached:u32,compare:u32,op:u32,early:u32,discard:u32,draws:u32,mask:u32,split:u32,checker:u32,reference:u32,read:u32,write:u32,negative:u32 }
impl Case { fn visible(&self)->bool{self.id>=10} }
fn cases()->Vec<Case>{
    let names=["no-stencil-early","always-keep-early","checker-keep-early","checker-replace-early","checker-zero-early","no-stencil-default","always-keep-default","checker-keep-default","checker-replace-default","checker-zero-default","visible-no-stencil-early","visible-always-keep-early","visible-checker-keep-early"];
    names.into_iter().enumerate().map(|(id,name)|{
        let role=if id>=10{id-10}else{id%5};let checker=u32::from(role>=2);
        Case{id,name,attached:u32::from(role!=0),compare:u32::from(checker==1),
            op:if role==3{2}else if role==4{1}else{0},early:u32::from(id<5||id>=10),discard:0,draws:1,mask:15,split:0,checker,
            reference:if checker==1{116}else{117},read:if checker==1{1}else{u32::MAX},
            write:if role>=3{15}else{u32::MAX},negative:0}
    }).collect()
}
fn tsv(cs:&[Case])->String{
    let mut out=String::from(HEADER);
    for c in cs {out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",c.id,c.name,c.attached,c.compare,c.op,c.early,c.discard,c.draws,c.mask,c.split,c.checker,c.reference,c.read,c.write,c.negative));}
    out
}
fn masked_rg16()->Vec<u8>{[0x00,0x34,0x00,0x3a].repeat(N)}
fn unorm(v:f32)->u8{if v<=0.0{0}else if v>=1.0{255}else if v==0.25{64}else if v==0.75{191}else{assert_eq!(v,0.5);128}}
fn seed()->Vec<u8>{
    let mut b=Vec::with_capacity(N*4);
    for y in 0..H {for x in 0..W {
        let a=((x*13+y*7+x*y*3)^(x>>1)^(y>>1))&1;
        b.extend([((x^y)&1)as u8*255,(y&1)as u8*255,(x&1)as u8*255,a as u8*255]);
    }}b
}
fn covered(x:usize,y:usize)->bool{(3..21).contains(&x)&&(5..20).contains(&y)}
fn primitive(x:usize,y:usize)->usize{
    // The odd height/even width guarantee no pixel center lies on the diagonal.
    usize::from(15*(2*x+1-6)+18*(2*y+1-10)>540)
}
fn initial_stencil(c:&Case)->Vec<u8>{
    (0..N).map(|i|if c.checker==1{42+(((i%W)+(i/W))&1)as u8}else{43}).collect()
}
fn passes(c:&Case,s:u8)->bool{c.attached==0||c.compare==0||((u32::from(s)&c.read)!=(c.reference&c.read))}
fn update(c:&Case,s:u8)->u8{
    let value=match c.op{0=>u32::from(s),1=>0,2=>c.reference,_=>unreachable!()};
    ((u32::from(s)&!c.write)|(value&c.write))as u8
}
#[derive(Clone,Debug,PartialEq)]
struct Model {color:Vec<u8>,a:Vec<u32>,b:Vec<u32>,stencil:Vec<u8>}
#[derive(Clone,Copy)]
enum Counter { None, PerPrimitive, ZeroRejectedHelpers, AllQuadStores }
fn model(c:&Case,counter:Counter,_reserved:bool)->Model{
    let mut m=Model{color:seed(),a:if c.visible(){words(&masked_rg16())}else{vec![(-999.0f32).to_bits();N*4]},b:if c.visible(){vec![]}else{vec![(-999.0f32).to_bits();N*4]},stencil:initial_stencil(c)};
    for draw in 0..c.draws {
        let snapshot=m.color.clone();let stencil_before=m.stencil.clone();
        for prim in 0..2 {
            let image=match counter{Counter::PerPrimitive=>m.color.clone(),_=>snapshot.clone()};
            let h=|x:usize,y:usize|{
                if matches!(counter,Counter::ZeroRejectedHelpers)&&!passes(c,stencil_before[y*W+x]) {0.0f32}
                else{0.25+0.5*f32::from(image[(y*W+x)*4+3])/255.0}
            };
            for y in 5..20 {for x in 3..21 {
                if primitive(x,y)!=prim{continue}
                let i=y*W+x;let pass=passes(c,m.stencil[i]);
                if c.attached==1&&pass&&c.early==1{m.stencil[i]=update(c,m.stencil[i]);}
                let stores=pass||matches!(counter,Counter::AllQuadStores);
                if !stores {continue}
                let prior=f32::from(image[i*4+3])/255.0;
                let dx=h((x&!1)+1,y)-h(x&!1,y);let dy=h(x,(y&!1)+1)-h(x,y&!1);let fw=dx.abs()+dy.abs();
                let a=[prior,h(x,y),dx,dy];let b=[fw,fw,prim as f32,(draw+1)as f32];
                if !c.visible(){for k in 0..4 {m.a[i*4+k]=a[k].to_bits();m.b[i*4+k]=b[k].to_bits();}}
                // RGB storage order is BGRA. All source/output channels are0/255.
                if c.visible(){m.color[i*4]=unorm(fw);m.color[i*4+1]=unorm(0.5+dy);m.color[i*4+2]=unorm(0.5+dx);}
                else if c.mask==15 {m.color[i*4]=image[i*4];m.color[i*4+1]=image[i*4+1];m.color[i*4+2]=255-image[i*4+2];}
                if c.mask&1!=0 {m.color[i*4+3]=255-image[i*4+3];}
                if c.attached==1&&pass&&c.early==0 {m.stencil[i]=update(c,m.stencil[i]);}
            }}
        }
    }m
}
fn differences<T:PartialEq>(a:&[T],b:&[T])->usize{a.iter().zip(b).filter(|(a,b)|a!=b).count()+a.len().abs_diff(b.len())}
fn numeric_difference(a:&Model,b:&Model)->usize{
    differences(&a.a,&b.a)+(0..N).map(|i|differences(&a.b[i*4..i*4+2],&b.b[i*4..i*4+2])).sum::<usize>()
}
fn error(s:impl Into<String>)->io::Error{io::Error::other(s.into())}
fn plane(raw:&[u8],at:&mut usize,bpp:usize)->io::Result<Vec<u8>>{
    let pitch=(W*bpp).div_ceil(256)*256;let bytes=2*GUARD+pitch*H;
    let b=raw.get(*at..*at+bytes).ok_or_else(||error("short guarded readback"))?;*at+=bytes;
    for (i,v)in b.iter().enumerate(){let payload=i>=GUARD&&i<GUARD+pitch*H&&(i-GUARD)%pitch<W*bpp;if !payload&&*v!=POISON{return Err(error("readback guard/padding modified"))}}
    Ok((0..H).flat_map(|y|b[GUARD+y*pitch..GUARD+y*pitch+W*bpp].iter().copied()).collect())
}
fn query(b:&[u8])->io::Result<Vec<u8>>{
    if b.len()!=N*4{return Err(error("query length"))}
    b.chunks_exact(4).map(|v|if v[1]==255-v[0]&&v[2]==0x53&&v[3]==255{Ok(v[0])}else{Err(error("incomplete GPU stencil query signature"))}).collect()
}
fn words(b:&[u8])->Vec<u32>{b.chunks_exact(4).map(|v|u32::from_le_bytes(v.try_into().unwrap())).collect()}
fn observed(root:&Path,c:&Case)->io::Result<Model>{
    let raw=fs::read(root.join(format!("case{:02}.bin",c.id)))?;let mut at=0;
    let before=plane(&raw,&mut at,4)?;let before_stencil=query(&plane(&raw,&mut at,4)?)?;
    if before!=seed()||before_stencil!=initial_stencil(c){return Err(error(format!("case{} initial GPU bytes differ",c.id)))}
    let color=plane(&raw,&mut at,4)?;let a=words(&plane(&raw,&mut at,if c.visible(){4}else{16})?);let b=if c.visible(){vec![]}else{words(&plane(&raw,&mut at,16)?)};let stencil=query(&plane(&raw,&mut at,4)?)?;
    if at!=raw.len(){return Err(error("unexpected native tail"))}Ok(Model{color,a,b,stencil})
}
fn prepare(root:&Path)->io::Result<()>{
    if root.exists(){return Err(error("inputs output must be new"))}fs::create_dir_all(root)?;
    fs::write(root.join("cases.tsv"),tsv(&cases()))?;fs::write(root.join("seed-bgra.bin"),seed())?;
    println!("PREPARED_OWNED_INPUTS cases=13 pixels=1024 native_executed=false");Ok(())
}
fn check(root:&Path,wrong:bool)->io::Result<()>{
    let cs=cases();if fs::read_to_string(root.join("cases.tsv"))?!=tsv(&cs)||fs::read(root.join("seed-bgra.bin"))?!=seed(){return Err(error("owned input mismatch"))}
    let cal=fs::read(root.join("query-calibration.bin"))?;let mut at=0;let q=query(&plane(&cal,&mut at,4)?)?;
    if at!=cal.len()||q!=(0..N).map(|i|(i&255)as u8).collect::<Vec<_>>(){return Err(error("all256 GPU stencil query calibration failed"))}
    let mut got=Vec::new();for c in &cs{got.push(observed(root,c)?)}
    let mut rows=Vec::new();let mut failures=0usize;
    for c in &cs {
        let mut expected_case=c.clone();if wrong{expected_case.compare=0;}
        let expected=model(&expected_case,Counter::None,false);let obs=&got[c.id];
        let dif=[differences(&obs.color,&expected.color),differences(&obs.a,&expected.a),differences(&obs.b,&expected.b),differences(&obs.stencil,&expected.stencil)];
        failures+=dif.iter().sum::<usize>();
        rows.push(format!("{{\"id\":{},\"name\":\"{}\",\"color_byte_mismatch\":{},\"visible_color_topology\":{},\"masked_RG16_word_mismatch\":{},\"diagnosticA_word_mismatch\":{},\"diagnosticB_word_mismatch\":{},\"stencil_byte_mismatch\":{}}}",c.id,c.name,dif[0],c.visible(),if c.visible(){dif[1]}else{0},if c.visible(){0}else{dif[1]},dif[2],dif[3]));
    }
    let paired=[(0,1),(5,6),(10,11)].iter().all(|&(a,b)|got[a].color==got[b].color&&got[a].a==got[b].a&&got[a].b==got[b].b);
    let primitive=numeric_difference(&got[1],&model(&cs[1],Counter::PerPrimitive,false));
    let rejected=numeric_difference(&got[2],&model(&cs[2],Counter::ZeroRejectedHelpers,false));
    let stores=differences(&got[2].color,&model(&cs[2],Counter::AllQuadStores,false).color);
    let visible_primitive=differences(&got[11].color,&model(&cs[11],Counter::PerPrimitive,false).color);
    let visible_rejected=differences(&got[12].color,&model(&cs[12],Counter::ZeroRejectedHelpers,false).color);
    let visible_mask=(10..13).all(|i|got[i].a==words(&masked_rg16())&&got[i].b.is_empty());
    let controls=paired&&primitive>0&&rejected>0&&stores>0&&visible_primitive>0&&visible_rejected>0&&visible_mask;
    let pass=failures==0&&controls;
    let report=format!("{{\"status\":\"{}\",\"native_model_qualification\":{},\"deliberately_wrong_ignore_stencil_model\":{},\"model\":\"pre-DRAW image for all derivative lanes; stores coverage&stencil_pass; ordinary pass operations; no discard\",\"default_means\":\"no early attribute, not forced hardware late execution\",\"rows\":[{}],\"paired_no_stencil_always_keep_equal\":{},\"wrong_per_primitive_differences\":{},\"wrong_zero_rejected_helpers_differences\":{},\"wrong_all_quad_store_color_differences\":{},\"visible_wrong_per_primitive_color_differences\":{},\"visible_wrong_zero_rejected_helpers_color_differences\":{},\"visible_RG16_masked_attachment_unchanged\":{},\"topology_qualification_separate\":true,\"platform_scope\":\"exact native.json OS build/device/compiler only; filenames are not build evidence\"}}\n",if pass{"NATIVE_MODEL_MATCH_WITH_NEGATIVE_WITNESSES"}else{"MODEL_MISMATCH_OR_CONTROL_NOT_DISCRIMINATING"},pass,wrong,rows.join(","),paired,primitive,rejected,stores,visible_primitive,visible_rejected,visible_mask);
    fs::write(root.join(if wrong{"negative-check.json"}else{"model-check.json"}),report)?;
    if !pass{return Err(error(format!("model mismatches={failures}; controls={controls}; native evidence retained; do not broaden qualification")))}
    println!("NATIVE_MODEL_MATCH cases=13 exact_tolerance=0");Ok(())
}
fn self_test()->io::Result<()>{
    let cs=cases();let m:Vec<_>=cs.iter().map(|c|model(c,Counter::None,false)).collect();
    assert_eq!(m[0],m[1]);assert_eq!(m[5],m[6]);assert_eq!(m[1],m[6]);assert_eq!(m[10],m[11]);
    assert_eq!(m[2].color,m[3].color);assert_eq!(m[2].color,m[4].color);
    assert!(differences(&m[2].stencil,&m[3].stencil)>0);assert!(differences(&m[2].stencil,&m[4].stencil)>0);
    let witnesses=[numeric_difference(&m[1],&model(&cs[1],Counter::PerPrimitive,false)),numeric_difference(&m[2],&model(&cs[2],Counter::ZeroRejectedHelpers,false)),differences(&m[2].color,&model(&cs[2],Counter::AllQuadStores,false).color)];
    let visible_witnesses=[differences(&m[11].color,&model(&cs[11],Counter::PerPrimitive,false).color),differences(&m[12].color,&model(&cs[12],Counter::ZeroRejectedHelpers,false).color)];
    assert!(visible_witnesses.iter().all(|n|*n>0));assert_eq!(m[12].a,words(&masked_rg16()));assert!(m[12].b.is_empty());
    assert!(witnesses.iter().all(|n|*n>0));assert_eq!((0..N).filter(|i|covered(i%W,i/W)).count(),270);
    println!("HOST_MODEL_SELF_TEST_PASS native_executed=false cases=13 witness_counts={witnesses:?} visible_color_witness_counts={visible_witnesses:?}");Ok(())
}
fn main(){
    let a:Vec<_>=env::args_os().collect();let r=match a.get(1).and_then(|s|s.to_str()){
        Some("--self-test")if a.len()==2=>self_test(),
        Some("prepare")if a.len()==3=>prepare(Path::new(&a[2])),
        Some("check")if a.len()==3=>check(Path::new(&a[2]),false),
        Some("check-wrong")if a.len()==3=>check(Path::new(&a[2]),true),
        _=>Err(error("usage: stencil-check --self-test | prepare NEW_RESULTS | check RESULTS | check-wrong RESULTS"))};
    if let Err(e)=r{eprintln!("{e}");std::process::exit(3)}
}
