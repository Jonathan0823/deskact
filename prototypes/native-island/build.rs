use std::{env, path::PathBuf};

fn main() {
    for path in [
        "acrylic-shim/DeskActAcrylicShim.cpp",
        "acrylic-shim/DeskActAcrylicShim.vcxproj",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let target = env::var("TARGET").unwrap_or_default();
    if target.ends_with("windows-msvc") {
        let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let shim_output = manifest_dir.join("acrylic-shim/bin/x64/Release");
        println!("cargo:rustc-link-search=native={}", shim_output.display());
        println!("cargo:rustc-link-lib=dylib=DeskActAcrylicShim");
    }
}
