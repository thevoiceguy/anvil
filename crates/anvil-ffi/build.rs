//! Generate `anvil.h` from the public extern "C" surface.
//!
//! Output goes to `OUT_DIR/anvil.h`. Mobile / desktop wrappers can
//! either pick it up from there during their own build or commit a
//! snapshot — once the API stabilises we'll publish a header alongside
//! each crate release.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let header_path = out_dir.join("anvil.h");

    // Skip header generation if cbindgen errors out (e.g. cross-compiling
    // without the host's libclang). Build still proceeds; the header is a
    // convenience artifact, not a build dependency.
    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .unwrap_or_default();

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&header_path);
            println!(
                "cargo:warning=anvil-ffi: wrote header to {}",
                header_path.display()
            );
        }
        Err(e) => {
            println!("cargo:warning=anvil-ffi: cbindgen failed: {e}");
        }
    }
}
