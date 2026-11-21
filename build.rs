use std::{env, fs, path::PathBuf, process::Command};
const WINDOW_ICON: &[u8] = include_bytes!("assets/brand/drakops.png");

fn word(out: &mut Vec<u8>, value:u16) { out.extend(value.to_le_bytes()); }

fn record(out: &mut Vec<u8>, kind:u16, id:u16, data:&[u8]) {
    let flags = if kind == 0 {0} else {0x1030};
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    for value in [0xffff,kind,0xffff,id] {word(out,value);}
    out.extend(0u32.to_le_bytes());
    word(out,flags);
    word(out,0); // Language-neutral.
    out.extend(0u64.to_le_bytes());
    out.extend(data);
    while !out.len().is_multiple_of(4) {out.push(0);}
}

fn main() {
    println!("cargo:rerun-if-changed=assets/brand/drakops.png");
    if env::var("CARGO_CFG_TARGET_OS").as_deref()!=Ok("windows"){return;}
    let out=PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let mut group=vec![0,0,1,0,1,0,0,0,0,0,1,0,32,0]; // One 256-pixel icon.
    group.extend((WINDOW_ICON.len() as u32).to_le_bytes());
    word(&mut group,1);
    let mut bytes=Vec::new();
    record(&mut bytes,0,0,&[]);
    record(&mut bytes,3,1,WINDOW_ICON);
    record(&mut bytes,14,1,&group);
    let resource=out.join("drakops.res");
    fs::write(&resource,bytes).expect("write icon resource");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref()==Ok("msvc"){
        println!("cargo:rustc-link-arg={}",resource.display());
        return;
    }
    let object=out.join("drakops-icon.o");
    let tool=env::var_os("WINDRES").unwrap_or_else(||"windres".into());
    println!("cargo:rerun-if-env-changed=WINDRES");
    match Command::new(tool).args(["-J","res","-O","coff","-i"]).arg(&resource).arg("-o").arg(&object).status(){
        Ok(status) if status.success()=>println!("cargo:rustc-link-arg={}",object.display()),
        error=>println!("cargo:warning=Windows executable icon not embedded ({error:?}); the app window icon is still available."),
    }
}
