//! neochumby-pajamasam: screensaver clips of the owner's own Pajama Sam
//! games, made on their server (src/convert.rs, docs/pajamasam.md).

#[cfg(not(all(target_arch = "arm", target_os = "linux")))]
fn main() {
    neochumby_scummvm::convert::main()
}

// The converter runs ScummVM headless on a server; it has no place on the
// Chumby.
#[cfg(all(target_arch = "arm", target_os = "linux"))]
fn main() {
    eprintln!("neochumby-pajamasam runs on a server (Linux or macOS), not on the Chumby");
    std::process::exit(1);
}
