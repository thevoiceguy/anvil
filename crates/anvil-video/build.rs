//! libvpx's bindings, on Linux: made from the installed headers by a
//! current bindgen, so they follow the system's libvpx and clang. libvpx
//! checks the ABI version each encoder and decoder is made with, so a
//! library that does not match fails there, safely. Bindings without the
//! encoder's configuration fail the build here, saying why.
//!
//! libvpx is linked statically where its archive is installed (Debian's
//! and Ubuntu's `libvpx-dev` ship one, built position-independent), so the
//! app's packages and AppImage need no libvpx of a particular soname on
//! the machine they run on; else, dynamically.

use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let vpx = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("vpx")
        .expect("libvpx (libvpx-dev) for VP8");
    link(&vpx);
    let mut builder = bindgen::Builder::default()
        .header_contents(
            "anvil_vpx.h",
            "#include <vpx/vpx_encoder.h>\n\
             #include <vpx/vpx_decoder.h>\n\
             #include <vpx/vp8cx.h>\n\
             #include <vpx/vp8dx.h>\n",
        )
        .allowlist_function("vpx_.*")
        .allowlist_type("vpx_.*|vp8e_.*")
        .allowlist_var("VPX_.*")
        .rustified_enum("vpx_.*|vp8e_.*")
        .layout_tests(false)
        .generate_comments(false);
    for dir in &vpx.include_paths {
        builder = builder.clang_arg(format!("-I{}", dir.display()));
    }
    let bindings = builder.generate().expect("libvpx's bindings");
    let text = bindings.to_string();
    if !text.contains("pub g_w:") {
        let at = text.find("struct vpx_codec_enc_cfg").unwrap_or(0);
        panic!(
            "libvpx's bindings have no encoder configuration (clang {:?}, includes {:?}):\n{}",
            bindgen::clang_version().full,
            vpx.include_paths,
            &text[at..(at + 600).min(text.len())]
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("vpx.rs");
    std::fs::write(out, text).expect("write libvpx's bindings");
}

/// Link libvpx: its archive where it is installed, else the shared library.
fn link(vpx: &pkg_config::Library) {
    let mut dirs = vpx.link_paths.clone();
    if let Ok(dir) = pkg_config::get_variable("vpx", "libdir") {
        dirs.push(PathBuf::from(dir));
    }
    for dir in &vpx.link_paths {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    match dirs.iter().find(|d| d.join("libvpx.a").is_file()) {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=vpx");
            // What the archive needs of the system.
            println!("cargo:rustc-link-lib=m");
            println!("cargo:rustc-link-lib=pthread");
        }
        None => {
            println!("cargo:warning=no libvpx.a: libvpx is linked dynamically");
            for lib in &vpx.libs {
                println!("cargo:rustc-link-lib={lib}");
            }
        }
    }
}
