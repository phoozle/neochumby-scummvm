// Builds ScummVM (third_party/scummvm: the sources in
// csrc/scummvm-sources.txt, configured as csrc/config/ says) and our
// backend (csrc/) into the binary.
//
// For the Chumby (armv5te-unknown-linux-musleabi) the compiler is zig's C++
// (tools/zigcxx), as zig is neochumby's C compiler and linker, and the
// binary links zig's libc++. Anywhere else it is the host's C++ compiler,
// for the host test mode.

use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap();
    let chumby = target.starts_with("armv5te-") && target.contains("linux");
    // neochumbyd's server (static musl, through zig): the converter.
    let server_musl = target == "x86_64-unknown-linux-musl";
    let scummvm = root.join("third_party/scummvm");

    let list = std::fs::read_to_string(root.join("csrc/scummvm-sources.txt")).unwrap();
    let mut files: Vec<PathBuf> =
        list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| scummvm.join(l)).collect();
    files.extend(["chumby-backend.cpp", "chumby-graphics.cpp"].map(|f| root.join("csrc").join(f)));

    let mut build = cc::Build::new();
    build.cpp(true).files(&files);
    if chumby {
        // zig gets the target and CPU from the wrapper; cc's own target
        // flags (--target, -march) would only contradict them.
        build
            .compiler(root.join("tools/zigcxx"))
            .archiver(zig_ar(&root))
            .cpp_link_stdlib(None)
            .no_default_flags(true)
            .flag("-O2")
            .flag("-ffunction-sections")
            .flag("-fdata-sections")
            .define("ARM_TARGET", None)
            .include(root.join("csrc/config/arm"));
    } else if server_musl && std::env::var_os("CXX_x86_64_unknown_linux_musl").is_none() {
        build
            .compiler(root.join("tools/zigcxx-x86_64"))
            .archiver(root.join("tools/zigar"))
            .cpp_link_stdlib(None)
            .opt_level(2)
            .include(root.join("csrc/config/host"));
    } else {
        build.opt_level(2).include(root.join("csrc/config/host"));
    }
    build
        .include(root.join("csrc/config"))
        .include(&scummvm)
        .include(scummvm.join("engines"))
        .include(root.join("csrc"))
        // As ScummVM's own configure and make build it (config.mk).
        .std("c++11")
        .flag("-fno-exceptions")
        .define("HAVE_CONFIG_H", None)
        .define("RELEASE_BUILD", None)
        .define("POSIX", None)
        .define("HAS_FSEEKO_OFFT_64", None)
        .define("DATA_PATH", "\"/nonexistent\"")
        .define("PLUGIN_DIRECTORY", "\"/nonexistent\"")
        .define("ENABLE_SCUMM", "STATIC_PLUGIN")
        .define("ENABLE_HE", None)
        // Upstream's warnings are upstream's.
        .warnings(false)
        .flag_if_supported("-w")
        .compile("scummvm");

    if chumby || server_musl {
        // zig's libc++, which `zig cc` (neochumby's linker) links on -lc++.
        println!("cargo:rustc-link-lib=c++");
    }
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=third_party/scummvm");
    println!("cargo:rerun-if-changed=tools/zigcxx");
}

/// zig's ar, unless AR for the target is set (tools/tree sets it to
/// neochumby's tools/zigar).
fn zig_ar(root: &Path) -> PathBuf {
    std::env::var_os("AR_armv5te_unknown_linux_musleabi").map(PathBuf::from).unwrap_or_else(|| root.join("tools/zigar"))
}
