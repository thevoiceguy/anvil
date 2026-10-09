//! Android: cpal's audio is oboe, C++ linked against the NDK's static C++
//! library. In the NDK the C++ ABI (`__cxa_pure_virtual` and the rest) is a
//! separate `libc++abi.a` that `clang++` adds by itself; Rust links with
//! `clang`, so without this the library builds and then fails to load.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Not `static=`: rustc would look for it itself; as `-lc++abi` the
        // NDK's clang finds the archive in its sysroot.
        println!("cargo:rustc-link-lib=c++abi");
    }
}
