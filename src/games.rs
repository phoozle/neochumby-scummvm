//! Which game to run, where its saves go, and ScummVM's command line.

use std::path::{Path, PathBuf};

/// The files that make a directory a SCUMM game: an HE game's index
/// (`PAJAMA2.HE0`), or an older game's (`000.LFL`, `MONKEY.000`, ...).
fn is_game(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        n.ends_with(".he0") || n.ends_with(".000") || n.ends_with(".la0") || n.ends_with(".sm0") || n == "000.lfl"
    })
}

/// The directory the games are in: the app's own (beside the binary),
/// unless NEOCHUMBY_SCUMMVM_GAMES says otherwise.
fn games_dir(argv0: &str) -> PathBuf {
    if let Some(d) = std::env::var_os("NEOCHUMBY_SCUMMVM_GAMES") {
        return d.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| Path::new(argv0).parent().unwrap_or(Path::new(".")).to_path_buf())
}

/// The games: each directory of the games directory holding one.
fn find(dir: &Path) -> Vec<String> {
    let mut games: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && is_game(&e.path()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    games.sort();
    games
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".neochumby-scummvm-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Bytes free on the filesystem holding `dir`, if it can be asked. (The
/// casts are needed on some targets, not others: statvfs's fields differ.)
#[allow(clippy::unnecessary_cast)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs(3) reads the NUL-terminated path and fills the
    // zeroed struct it is given, both live for the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// /psp is the Chumby's own settings flash, 2 MB in all; a save of an HE
/// game is about 200 KB. Saves go there only while this much stays free,
/// so they can never fill it and stop the Chumby saving its settings.
const PSP_KEEP_FREE: u64 = 1024 * 1024;

/// Where saves go, the first of these that can be written:
///
/// 1. NEOCHUMBY_SCUMMVM_SAVES;
/// 2. `saves/` in the games directory, when it is writable (a stick
///    mounted read-write): saves stay with the games;
/// 3. `/psp/scummvm`: the Chumby's own flash, which keeps them across
///    reboots, while it has room ([`PSP_KEEP_FREE`]);
/// 4. `$HOME/scummvm-saves` (HOME is /tmp, in RAM): lost at power-off.
fn saves_dir(games: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    if let Some(d) = std::env::var_os("NEOCHUMBY_SCUMMVM_SAVES").map(PathBuf::from)
        && writable(&d)
    {
        return d;
    }
    let stick = games.join("saves");
    if writable(&stick) {
        return stick;
    }
    let psp = PathBuf::from("/psp/scummvm");
    if Path::new("/psp").is_dir() && free_bytes(Path::new("/psp")).is_some_and(|f| f >= PSP_KEEP_FREE) && writable(&psp)
    {
        return psp;
    }
    home.join("scummvm-saves")
}

/// ScummVM's arguments: the picked game's directory, auto-detected, with
/// its saves and ScummVM's configuration file somewhere writable.
pub fn command_line(argv0: &str) -> Result<Vec<String>, String> {
    let dir = games_dir(argv0);
    let games = find(&dir);
    let game = match games.len() {
        0 => {
            return Err(format!(
                "no games in {}: put each game's files in a directory of its own there (pajama2/PAJAMA2.HE0 ...)",
                dir.display()
            ));
        }
        1 => &games[0],
        _ => &games[crate::platform::pick(&games)],
    };
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    let saves = saves_dir(&dir);
    eprintln!("neochumby-scummvm: {game}, saves in {}", saves.display());
    let mut args = vec![
        argv0.to_string(),
        format!("--config={}", home.join("scummvm-chumby.ini").display()),
        format!("--savepath={}", saves.display()),
        format!("--path={}", dir.join(game).display()),
    ];
    // More options for ScummVM (`-d2`, `--language=de`), space-separated.
    if let Ok(extra) = std::env::var("NEOCHUMBY_SCUMMVM_ARGS") {
        args.extend(extra.split_whitespace().map(String::from));
    }
    args.push("--auto-detect".into());
    Ok(args)
}

/// Shows why nothing could be run, for a few seconds, on the Chumby.
pub fn show_error(msg: &str) {
    #[cfg(all(target_arch = "arm", target_os = "linux"))]
    {
        use neochumby::fb::{self, rgb};
        crate::abi::with_platform(|p| {
            let Some(fb) = p.fb() else { return };
            fb.clear(rgb(48, 0, 0));
            // 8x8 text, wrapped at the screen's 40 columns.
            let mut y = 8;
            for line in msg.as_bytes().chunks(fb::W / 8) {
                fb.text(0, y, &String::from_utf8_lossy(line), 1, rgb(255, 255, 255));
                y += 10;
            }
        });
        crate::platform::sleep_ms(8000);
    }
    let _ = msg;
}
