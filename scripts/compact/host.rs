use std::{env,fs,io,path::Path,process::{Command,Stdio},time::{Duration,Instant}};
extern "C" {
    fn compact_probe(bytes:*const u8,length:usize,rounding:*mut i64,threads:*mut u64,json:*mut u8,capacity:usize)->i32;
}
// Owned minimal descriptors. No functions, libraries, references or captured bytes.
const CASES: &[(&[u8], i64, u64)] = &[
    (&[0],0,0),
    (&[1,10,4,0,0,0,0],0,0),
    (&[1,10,4,1,0,0,0],1,0),
    (&[1,10,4,2,0,0,0],2,0),
    (&[1,8,4,2,0,0,0],0,2), // Same value, different tag: must not change rounding.
    (&[2,8,4,32,0,0,0,10,4,2,0,0,0],2,32),
    (&[2,10,4,2,0,0,0,8,4,32,0,0,0],2,32),
];
fn child(index:usize)->io::Result<i32> {
    let &(bytes,want_rounding,want_threads)=CASES.get(index).ok_or_else(||io::Error::other("case index"))?;
    let mut rounding=i64::MIN;let mut threads=u64::MAX;let mut json=[0u8;16384];
    // SAFETY: bridge reads only this immutable slice and writes to live output
    // objects/bounded JSON. No native-owned pointer escapes the synchronous call.
    let status=unsafe{compact_probe(bytes.as_ptr(),bytes.len(),&mut rounding,&mut threads,json.as_mut_ptr(),json.len())};
    let end=json.iter().position(|&b|b==0).ok_or_else(||io::Error::other("JSON terminator"))?;
    println!("{}",std::str::from_utf8(&json[..end]).map_err(io::Error::other)?);
    Ok(if status==0 && rounding==want_rounding && threads==want_threads {0}else{2})
}
fn run()->io::Result<i32> {
    let args:Vec<_>=env::args().collect();
    if args.len()==3 && args[1]=="--child" {return child(args[2].parse().map_err(io::Error::other)?)}
    if args.len()!=1 {return Err(io::Error::other("no arguments expected"))}
    let out=Path::new("out-compact");fs::create_dir(out)?;
    let mut passed=0;
    for (i,(input,_,_)) in CASES.iter().enumerate() {
        fs::write(out.join(format!("case-{i}.input.bin")),input)?;
        let mut p=Command::new(env::current_exe()?).args(["--child",&i.to_string()])
            .stdin(Stdio::null()).stdout(fs::File::create(out.join(format!("case-{i}.native.json")))?)
            .stderr(Stdio::null()).spawn()?;
        let start=Instant::now();let mut timeout=false;
        let status=loop{if let Some(s)=p.try_wait()?{break s}if start.elapsed()>Duration::from_secs(15){timeout=true;p.kill()?;break p.wait()?}std::thread::sleep(Duration::from_millis(50))};
        let ok=status.success()&&!timeout;passed+=usize::from(ok);
        fs::write(out.join(format!("case-{i}.process.json")),format!("{{\"expected_property_checks_pass\":{ok},\"timeout\":{timeout},\"exit\":{}}}\n",status.code().map_or("null".into(),|n|n.to_string())))?;
    }
    fs::write(out.join("summary.json"),format!("{{\"cases\":{},\"passed\":{passed},\"scope\":\"owned descriptor fields through native reader; no pipeline or GPU execution\"}}\n",CASES.len()))?;
    Ok(if passed==CASES.len(){0}else{2})
}
fn main(){std::process::exit(match run(){Ok(n)=>n,Err(e)=>{eprintln!("{e}");1}})}
