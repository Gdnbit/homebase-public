use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let runner = root.join("src-tauri/src/adapters/process_runner.rs");
    let source = fs::read_to_string(&runner).unwrap();
    let start = source
        .find("use std::ffi::OsStr;")
        .expect("runner start marker");
    let end = source[start..]
        .find("#[cfg(test)]")
        .map(|i| start + i)
        .expect("runner end marker");
    fs::write(out.join("runner.rs"), &source[start..end]).unwrap();
    let sops = root.join("src-tauri/src/adapters/sops.rs");
    let source = fs::read_to_string(&sops).unwrap();
    // Compile production code, never the legacy tests that can read a real vault.
    let end = source.find("#[cfg(test)]").expect("legacy test boundary");
    let production = source[..end]
        .lines()
        .map(|line| {
            line.strip_prefix("//!")
                .map(|s| format!("//{s}"))
                .unwrap_or_else(|| line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(out.join("sops.rs"), production).unwrap();
    println!("cargo:rerun-if-changed={}", runner.display());
    println!("cargo:rerun-if-changed={}", sops.display());
}
