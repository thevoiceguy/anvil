//! libvpx's bindings, on Linux: made from the installed headers by a
//! current bindgen, so they follow the system's libvpx and clang. libvpx
//! checks the ABI version each encoder and decoder is made with, so a
//! library that does not match fails there, safely. Bindings without the
//! encoder's configuration fail the build here, saying why.

use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let vpx = pkg_config::Config::new()
        .probe("vpx")
        .expect("libvpx (libvpx-dev) for VP8");
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
