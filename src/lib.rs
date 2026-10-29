//! neochumby-scummvm: ScummVM's SCUMM engine (with the Humongous
//! Entertainment games: Pajama Sam and friends) on the Chumby Classic, as a
//! neochumby app.
//!
//! ScummVM (C++, third_party/scummvm) runs on a backend of ours
//! (csrc/chumby-*.cpp), which calls this crate for the devices through a
//! small C ABI (csrc/chumby.h, src/abi.rs), as neochumby's Doom does. Built
//! for the Chumby, the devices are neochumby's (src/device.rs); built for
//! anything else, they are the host test mode's stand-ins (src/host.rs).
//!
//! The games are the owner's own copies, one directory each beside the
//! binary (`apps/scummvm/pajama2/`). One is run at once; with more than
//! one, a list is shown to pick from.
//!
//! `neochumby-scummvm --scummvm ARGS...` runs ScummVM with ARGS as they
//! are, for debugging (`--scummvm --version`, `--scummvm --list-games`).

// FFI to a C++ engine (GPL), so allowed unsafe code, as neochumby's games
// are.
#![allow(unsafe_code)]

pub mod abi;
pub mod clip;
mod games;
pub mod input;

#[cfg(all(target_arch = "arm", target_os = "linux"))]
#[path = "device.rs"]
mod platform;
#[cfg(not(all(target_arch = "arm", target_os = "linux")))]
#[path = "host.rs"]
pub mod platform;

#[cfg(not(all(target_arch = "arm", target_os = "linux")))]
pub mod convert;

/// The app: neochumby-scummvm's main.
pub fn app_main() -> ! {
    #[cfg(all(target_arch = "arm", target_os = "linux"))]
    neochumby::debug::install("neochumby-scummvm");
    abi::init();

    let mut args: Vec<String> = std::env::args().collect();
    let code = if args.get(1).map(String::as_str) == Some("--scummvm") {
        args.remove(1);
        abi::scummvm_main(&args)
    } else {
        match games::command_line(&args[0]) {
            Ok(argv) => abi::scummvm_main(&argv),
            Err(e) => {
                eprintln!("neochumby-scummvm: {e}");
                games::show_error(&e);
                1
            }
        }
    };
    abi::finish();
    std::process::exit(code);
}
