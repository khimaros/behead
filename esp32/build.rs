use std::path::{Path, PathBuf};

/// maps put in this folder beside the sources go into the firmware
const MAPS: &str = "maps";
const MAP_ENDING: &str = "bmap";
/// the list of them the firmware is built with
const LIST: &str = "maps.rs";

fn main() {
    embuild::espidf::sysenv::output();
    println!("cargo:rerun-if-changed={MAPS}");
    let files = std::fs::read_dir(MAPS).into_iter().flatten().flatten().map(|entry| entry.path());
    let mut maps: Vec<PathBuf> = files.filter(|path| path.extension().is_some_and(|ending| ending == MAP_ENDING)).collect();
    maps.sort();
    let whole = |map: &PathBuf| format!("include_bytes!({:?})", std::fs::canonicalize(map).unwrap());
    let list = format!("const BUILT_IN_MAPS: &[&[u8]] = &[{}];\n", maps.iter().map(whole).collect::<Vec<_>>().join(", "));
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join(LIST), list).unwrap();
}
