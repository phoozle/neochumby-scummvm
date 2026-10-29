//! neochumby-pajamasam: makes screensaver clips (docs/clip-format.md) of
//! the owner's own Pajama Sam games, on their server. It runs ScummVM
//! headless (the host test mode, src/host.rs), one child process at a time
//! per job, enters rooms through ScummVM's debugger, lets them idle, and
//! records them; then it finds a loop in each recording and writes a clip.
//!
//! The command line, its progress lines and its exit codes are a contract
//! with the web page that runs it: docs/pajamasam.md.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::clip::{self, Clip, Frame};

const EXIT_USAGE: i32 = 1;
const EXIT_NO_GAME: i32 = 2;
const EXIT_NO_CLIP: i32 = 3;
const EXIT_OUT: i32 = 4;

const USAGE: &str = "usage:
  neochumby-pajamasam convert --out DIR [--ps1 DIR] [--ps2 DIR] [--scenes FILE] [--auto N]
                              [--fps N] [--seconds N] [--budget-mb N] [--jobs N]
  neochumby-pajamasam survey (--ps1 DIR | --ps2 DIR)
  neochumby-pajamasam check [--ps1 DIR] [--ps2 DIR]
  neochumby-pajamasam rooms (--ps1 DIR | --ps2 DIR)
  neochumby-pajamasam verify DIR
  neochumby-pajamasam encode RECORDING.raw --out DIR [--budget-mb N]   (a name from the file's)";

/// A game this tool knows: its short name, ScummVM's id, its index file,
/// the files it needs beside it (any one of a group), and when its intro
/// is over (Escape is pressed at these times, in ms, to skip it).
struct Game {
    short: &'static str,
    id: &'static str,
    index: &'static str,
    needs: &'static [&'static [&'static str]],
}

const GAMES: &[Game] = &[
    Game {
        short: "ps1",
        id: "pajama",
        index: "pajama.he0",
        needs: &[&["pajama.(a)", "pajama.he1"], &["pajama.he2"], &["pajama.he4"]],
    },
    Game {
        short: "ps2",
        id: "pajama2",
        index: "pajama2.he0",
        needs: &[&["pajama2.he1"], &["pajama2.he2"], &["pajama2.he4"]],
    },
];

/// When, after ScummVM starts, Escape is pressed (skipping the intro), and
/// when the first room may be entered.
const ESCAPES_MS: &[u32] = &[3000, 5000, 7000, 9000];
const FIRST_ROOM_MS: u32 = 11_000;

fn emit(line: String) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn quote(s: &str) -> String {
    if s.is_empty() || s.contains([' ', '"']) { format!("\"{}\"", s.replace('"', "'")) } else { s.to_string() }
}

/// A file in `dir` whose name, in lower case, is `lower`.
fn find_file(dir: &Path, lower: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase() == lower) && p.is_file())
}

/// Whether `dir` holds `game`; else why not.
fn recognise(game: &Game, dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    if find_file(dir, game.index).is_none() {
        return Err(format!("no {}", game.index.to_uppercase()));
    }
    for group in game.needs {
        if !group.iter().any(|f| find_file(dir, f).is_some()) {
            return Err(format!("no {}", group.join(" or ").to_uppercase()));
        }
    }
    Ok(())
}

/// The rooms a game's index names (its RNAM block): number and name. The
/// HE index files are XORed with 0x69; HE80 and later list a 16-bit number
/// and a NUL-terminated name, earlier ones a byte and 9 bytes XORed with
/// 0xFF. Metadata only: nothing of the game's art or sound.
pub fn rooms(index: &Path) -> Result<Vec<(u16, String)>, String> {
    let raw = std::fs::read(index).map_err(|e| format!("{}: {e}", index.display()))?;
    for key in [0x69u8, 0] {
        let d: Vec<u8> = raw.iter().map(|b| b ^ key).collect();
        let Some(i) = d.windows(4).position(|w| w == b"RNAM") else { continue };
        let size = u32::from_be_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
        let block = &d[i + 8..(i + size).min(d.len())];
        for wide in [true, false] {
            if let Some(r) = parse_rnam(block, wide)
                && !r.is_empty()
            {
                return Ok(r);
            }
        }
    }
    Err(format!("{}: no room names found", index.display()))
}

fn parse_rnam(b: &[u8], wide: bool) -> Option<Vec<(u16, String)>> {
    let mut out = Vec::new();
    let mut p = 0;
    loop {
        let room = if wide { u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]) } else { *b.get(p)? as u16 };
        p += if wide { 2 } else { 1 };
        if room == 0 {
            return Some(out);
        }
        let name: Vec<u8> = if wide {
            let n = b[p..].iter().position(|&c| c == 0)?;
            let s = b[p..p + n].to_vec();
            p += n + 1;
            s
        } else {
            let s: Vec<u8> = b.get(p..p + 9)?.iter().map(|c| c ^ 0xFF).take_while(|&c| c != 0).collect();
            p += 9;
            s
        };
        if name.iter().any(|c| !(0x20..0x7f).contains(c)) {
            return None;
        }
        out.push((room, String::from_utf8_lossy(&name).into_owned()));
    }
}

/// One run of ScummVM: the script's steps, then quit. Returns the
/// directory its recordings went to.
struct Run {
    work: PathBuf,
    script: Vec<String>,
}

impl Run {
    fn new(work: PathBuf) -> Run {
        let script = ESCAPES_MS.iter().map(|t| format!("{t} esc")).collect();
        Run { work, script }
    }

    /// Runs ScummVM on `game` in `dir` with the script; the exit status.
    fn exec(&mut self, game: &Game, dir: &Path, end_ms: u32) -> Result<(), String> {
        self.script.push(format!("{end_ms} quit"));
        std::fs::create_dir_all(self.work.join("home")).map_err(|e| format!("{}: {e}", self.work.display()))?;
        let script = self.work.join("script.txt");
        std::fs::write(&script, self.script.join("\n")).map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let home = self.work.join("home");
        let out = Command::new(exe)
            .arg("--child")
            .arg(format!("--config={}", home.join("scummvm.ini").display()))
            .arg(format!("--savepath={}", home.display()))
            .arg(format!("--path={}", dir.display()))
            .arg(game.id)
            .env("HOME", &home)
            .env("NEOCHUMBY_SCUMMVM_SCRIPT", &script)
            .env("NEOCHUMBY_SCUMMVM_OUT", &self.work)
            .env("NEOCHUMBY_SCUMMVM_NO_CURSOR", "1")
            .env_remove("NEOCHUMBY_SCUMMVM_WAV")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| format!("running ScummVM: {e}"))?;
        if out.success() { Ok(()) } else { Err(format!("ScummVM exited {}", out.code().unwrap_or(-1))) }
    }
}

/// A recording made by the host test mode (src/host.rs): its fps and frames.
fn read_raw(path: &Path) -> Result<(u32, Vec<Frame>), String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.len() < 14 || &d[..8] != b"NCRAW001" {
        return Err(format!("{}: not a recording", path.display()));
    }
    let fps = u16::from_le_bytes([d[8], d[9]]) as u32;
    let n = u32::from_le_bytes(d[10..14].try_into().unwrap()) as usize;
    let body = &d[14..];
    if body.len() != n * clip::PIXELS * 2 {
        return Err(format!("{}: {} bytes for {n} frames", path.display(), body.len()));
    }
    let frames = body
        .as_chunks::<{ clip::PIXELS * 2 }>()
        .0
        .iter()
        .map(|f| f.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect())
        .collect();
    Ok((fps, frames))
}

/// How lively a room is, from a few seconds of it: the median share of
/// pixels changing between frames, if the room is worth a clip (something
/// keeps moving, but not the whole picture, and it isn't mostly one
/// colour); else None.
pub fn liveliness(frames: &[Frame]) -> Option<f64> {
    if frames.len() < 3 {
        return None;
    }
    let mut c: Vec<f64> = frames.windows(2).map(|w| clip::changed(&w[0], &w[1])).collect();
    c.sort_by(f64::total_cmp);
    let median = c[c.len() / 2];
    let max = *c.last().unwrap();
    // Mostly one colour: a blank or fading screen.
    let last = frames.last().unwrap();
    let mut counts: BTreeMap<u16, usize> = BTreeMap::new();
    for p in last.iter().step_by(7) {
        *counts.entry(*p).or_default() += 1;
    }
    let top = counts.values().max().copied().unwrap_or(0) as f64 / (clip::PIXELS / 7) as f64;
    ((0.02..=0.6).contains(&median) && max <= 0.85 && top < 0.8).then_some(median)
}

/// The rooms of `game`, each recorded for a moment, ranked liveliest
/// first: (room, name, liveliness). Restarts ScummVM past a room that
/// stops it.
fn survey(game: &Game, dir: &Path, work: &Path) -> Vec<(u16, String, f64)> {
    let index = find_file(dir, game.index).unwrap();
    let all = rooms(&index).unwrap_or_default();
    let (settle, fps, frames) = (1500u32, 4u32, 12u32);
    let per_room = settle + frames * 1000 / fps + 500;
    let mut found = Vec::new();
    let mut next = 0;
    let mut restarts = 0;
    while next < all.len() && restarts < 5 {
        let mut run = Run::new(work.join(format!("survey-{}-{next}", game.short)));
        let mut t = FIRST_ROOM_MS;
        // At most 40 rooms a run, so that a crash costs little.
        let batch: Vec<usize> = (next..all.len()).take(40).collect();
        for &i in &batch {
            run.script.push(format!("{t} debug room {}", all[i].0));
            run.script.push(format!("{} record {fps} {frames} room-{}.raw", t + settle, all[i].0));
            t += per_room;
        }
        let ok = run.exec(game, dir, t);
        let mut last_done = None;
        for &i in &batch {
            let (room, ref name) = all[i];
            let path = run.work.join(format!("room-{room}.raw"));
            let Ok((_, f)) = read_raw(&path) else { break };
            last_done = Some(i);
            let l = liveliness(&f);
            emit(format!(
                "survey {} room={room} of={} name={} lively={}",
                game.short,
                all.len(),
                quote(name),
                l.map_or("no".into(), |v| format!("{v:.3}"))
            ));
            if let Some(v) = l {
                found.push((room, name.clone(), v));
            }
        }
        let _ = std::fs::remove_dir_all(&run.work);
        next = match last_done {
            Some(i) => i + 1,
            None => next + 1, // the first room of the batch stopped it: skip it
        };
        if ok.is_err() {
            restarts += 1;
        }
    }
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    found
}

/// A scene to record.
#[derive(Clone)]
pub struct Scene {
    pub name: String,
    pub game: String,
    pub room: u16,
    pub settle_ms: u32,
    pub seconds: u32,
    pub fps: u32,
    pub title: String,
}

/// The scene list: `NAME GAME ROOM SETTLE_MS SECONDS FPS TITLE...`, `#`
/// comments.
pub fn parse_scenes(text: &str) -> Result<Vec<Scene>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let bad = || format!("scenes line {}: {line}", n + 1);
        if f.len() < 6 {
            return Err(bad());
        }
        let num = |i: usize| f[i].parse::<u32>().map_err(|_| bad());
        let name = f[0].to_string();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(bad());
        }
        out.push(Scene {
            name,
            game: f[1].to_string(),
            room: num(2)? as u16,
            settle_ms: num(3)?,
            seconds: num(4)?.clamp(2, 60),
            fps: num(5)?.clamp(1, 50),
            title: f[6..].join(" "),
        });
    }
    Ok(out)
}

/// Records a scene and writes its clip; its stats line, or why not.
fn make_clip(game: &Game, dir: &Path, scene: &Scene, work: &Path, out: &Path, budget: u64) -> Result<String, String> {
    emit(format!("scene {} recording title={}", scene.name, quote(&scene.title)));
    let mut run = Run::new(work.join(format!("scene-{}", scene.name)));
    let n = scene.seconds * scene.fps;
    run.script.push(format!("{FIRST_ROOM_MS} debug room {}", scene.room));
    let start = FIRST_ROOM_MS + scene.settle_ms;
    run.script.push(format!("{start} record {} {n} scene.raw", scene.fps));
    let result = run.exec(game, dir, start + scene.seconds * 1000 + 500);
    let recorded = read_raw(&run.work.join("scene.raw"));
    let _ = std::fs::remove_dir_all(&run.work);
    let (fps, frames) = match (result, recorded) {
        (_, Ok(r)) => r,
        (Err(e), Err(_)) | (Ok(()), Err(e)) => return Err(e),
    };
    if liveliness(&frames).is_none() {
        return Err("the room didn't move (or was blank) while recorded".into());
    }
    let (clip, err, secs) = loop_clip(frames, fps, budget);
    let s = clip.stats();
    clip.write(out, &scene.name).map_err(|e| format!("{}: {e}", out.display()))?;
    let seconds = s.frames as f64 / s.fps as f64;
    let _ = secs;
    Ok(format!(
        "scene {} done frames={} fps={} loop={} seconds={seconds:.1} bytes={} max_record={} loop_error={err:.2}",
        scene.name, s.frames, s.fps, s.loop_to, s.bytes, s.max_record
    ))
}

/// The best loop in a recording, within the byte budget: cut where the
/// picture comes back nearest to where the loop starts, crossfaded if it
/// doesn't come back close; at half the fps, or shorter, if too big.
/// Returns the clip, the loop's distance at the cut, and its seconds.
pub fn loop_clip(mut frames: Vec<Frame>, mut fps: u32, budget: u64) -> (Clip, f64, f64) {
    // A cut nearer than this (mean difference per sampled pixel, of 0-765)
    // is invisible enough at a screensaver's distance; further, crossfade.
    const CLOSE: f64 = 4.0;
    loop {
        let fade = (fps / 2).max(1) as usize;
        let min_len = (fps * 3) as usize;
        let n = frames.len();
        let (mut a, b, d) = clip::find_loop(&frames[fade.min(n / 3)..], min_len.min(n / 2))
            .map(|(a, b, d)| (a + fade.min(n / 3), b + fade.min(n / 3), d))
            .unwrap_or((0, n, f64::MAX));
        let mut cut: Vec<Frame>;
        if d > CLOSE && a >= fade && b - a > fade * 2 {
            let mut fr = frames.clone();
            clip::crossfade(&mut fr, a, b, fade);
            cut = fr[a..b].to_vec();
        } else {
            cut = frames[a.min(n - 1)..b.min(n)].to_vec();
            if cut.is_empty() {
                cut = frames.clone();
                a = 0;
            }
        }
        let _ = a;
        let c = Clip::encode(&cut, fps as u16, 0);
        let bytes = c.stats().bytes;
        if bytes <= budget || cut.len() <= 8 {
            let secs = cut.len() as f64 / fps as f64;
            return (c, d, secs);
        }
        // Too big: half the frame rate while it stays smooth enough (6
        // fps), then half the length.
        if fps >= 12 {
            frames = frames.into_iter().step_by(2).collect();
            fps /= 2;
        } else {
            frames.truncate(frames.len() * 2 / 3);
        }
    }
}

/// `clips.txt`: every clip in `out` that reads back, with what the
/// previous list said of its game and title, and the new ones'.
fn write_list(out: &Path, new: &BTreeMap<String, (String, String)>) -> std::io::Result<()> {
    let list = out.join("clips.txt");
    let mut known: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Ok(old) = std::fs::read_to_string(&list) {
        for l in old.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() >= 6 {
                known.insert(f[0].into(), (f[1].into(), f[6..].join(" ")));
            }
        }
    }
    known.extend(new.clone());
    let mut text = String::from("# NAME GAME FPS FRAMES SECONDS BYTES TITLE (neochumby-pajamasam)\n");
    for (name, (game, title)) in &known {
        if let Ok(r) = clip::read(out, name) {
            let s = r.stats;
            text += &format!(
                "{name} {game} {} {} {:.1} {} {title}\n",
                s.fps,
                s.frames,
                s.frames as f64 / s.fps as f64,
                s.bytes
            );
        }
    }
    let tmp = out.join("clips.txt.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, list)
}

struct Opts {
    dirs: Vec<(&'static Game, PathBuf)>,
    out: Option<PathBuf>,
    scenes: Option<PathBuf>,
    auto: usize,
    fps: u32,
    seconds: u32,
    budget: u64,
    jobs: usize,
    rest: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        dirs: Vec::new(),
        out: None,
        scenes: None,
        auto: 4,
        fps: 12,
        seconds: 16,
        budget: 3 << 20,
        jobs: std::thread::available_parallelism().map_or(2, |n| n.get()),
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned().ok_or(format!("{a} needs a value"))
        };
        let num = |v: String| v.parse::<u64>().map_err(|_| format!("{a}: not a number: {v}"));
        match a {
            "--ps1" => o.dirs.push((&GAMES[0], val()?.into())),
            "--ps2" => o.dirs.push((&GAMES[1], val()?.into())),
            "--out" => o.out = Some(val()?.into()),
            "--scenes" => o.scenes = Some(val()?.into()),
            "--auto" => o.auto = num(val()?)? as usize,
            "--fps" => o.fps = num(val()?)?.clamp(1, 50) as u32,
            "--seconds" => o.seconds = num(val()?)?.clamp(2, 60) as u32,
            "--budget-mb" => o.budget = num(val()?)?.max(1) << 20,
            "--jobs" => o.jobs = num(val()?)?.max(1) as usize,
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ => o.rest.push(a.to_string()),
        }
        i += 1;
    }
    Ok(o)
}

/// The games given and recognised, with progress lines for each.
fn games(o: &Opts) -> Vec<(&'static Game, PathBuf)> {
    let mut ok = Vec::new();
    for (g, d) in &o.dirs {
        match recognise(g, d) {
            Ok(()) => {
                emit(format!("game {} found path={}", g.short, quote(&d.display().to_string())));
                ok.push((*g, d.clone()));
            }
            Err(e) => emit(format!("game {} missing reason={}", g.short, quote(&e))),
        }
    }
    ok
}

pub fn main() -> ! {
    let args: Vec<String> = std::env::args().collect();
    // A child: ScummVM itself, in the host test mode, as convert runs it.
    if args.get(1).map(String::as_str) == Some("--child") {
        crate::abi::init();
        let mut argv = vec![args[0].clone()];
        argv.extend(args[2..].iter().cloned());
        let code = crate::abi::scummvm_main(&argv);
        crate::abi::finish();
        std::process::exit(code);
    }
    let fail = |code: i32, msg: &str| -> ! {
        eprintln!("neochumby-pajamasam: {msg}");
        if code == EXIT_USAGE {
            eprintln!("{USAGE}");
        }
        std::process::exit(code)
    };
    let Some(cmd) = args.get(1) else { fail(EXIT_USAGE, "no command") };
    let o = parse(&args[2..]).unwrap_or_else(|e| fail(EXIT_USAGE, &e));
    match cmd.as_str() {
        "check" => {
            if o.dirs.is_empty() {
                fail(EXIT_USAGE, "check needs --ps1 or --ps2");
            }
            std::process::exit(if games(&o).is_empty() { EXIT_NO_GAME } else { 0 });
        }
        "rooms" => {
            let [(g, d)] = o.dirs.as_slice() else { fail(EXIT_USAGE, "rooms needs one of --ps1, --ps2") };
            if let Err(e) = recognise(g, d) {
                fail(EXIT_NO_GAME, &e);
            }
            match rooms(&find_file(d, g.index).unwrap()) {
                Ok(r) => r.iter().for_each(|(n, name)| emit(format!("room {} {n} {}", g.short, quote(name)))),
                Err(e) => fail(EXIT_NO_GAME, &e),
            }
            std::process::exit(0);
        }
        "verify" => {
            let [dir] = o.rest.as_slice() else { fail(EXIT_USAGE, "verify needs a directory") };
            std::process::exit(verify(Path::new(dir)));
        }
        "survey" => {
            let [(g, d)] = o.dirs.as_slice() else { fail(EXIT_USAGE, "survey needs one of --ps1, --ps2") };
            if let Err(e) = recognise(g, d) {
                fail(EXIT_NO_GAME, &e);
            }
            let work = std::env::temp_dir().join(format!("neochumby-pajamasam-{}", std::process::id()));
            for (room, name, l) in survey(g, d, &work) {
                emit(format!("lively {} room={room} name={} lively={l:.3}", g.short, quote(&name)));
            }
            let _ = std::fs::remove_dir_all(&work);
            std::process::exit(0);
        }
        "encode" => {
            let ([raw], Some(out)) = (o.rest.as_slice(), o.out.as_ref()) else {
                fail(EXIT_USAGE, "encode needs a recording and --out")
            };
            let (fps, frames) = read_raw(Path::new(raw)).unwrap_or_else(|e| fail(EXIT_USAGE, &e));
            let name = Path::new(raw).file_stem().unwrap().to_string_lossy().into_owned();
            std::fs::create_dir_all(out).unwrap_or_else(|e| fail(EXIT_OUT, &e.to_string()));
            let (c, d, _) = loop_clip(frames, fps, o.budget);
            c.write(out, &name).unwrap_or_else(|e| fail(EXIT_OUT, &e.to_string()));
            let s = c.stats();
            emit(format!(
                "clip {name} done frames={} fps={} loop={} bytes={} max_record={} loop_error={d:.2}",
                s.frames, s.fps, s.loop_to, s.bytes, s.max_record
            ));
            std::process::exit(0);
        }
        "convert" => std::process::exit(convert(&o).unwrap_or_else(|(c, m)| fail(c, &m))),
        _ => fail(EXIT_USAGE, &format!("unknown command {cmd}")),
    }
}

fn verify(dir: &Path) -> i32 {
    let mut bad = 0;
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".idx").map(String::from))
        .collect();
    names.sort();
    for n in &names {
        match clip::read(dir, n).and_then(|r| r.play().map(|_| r.stats)) {
            Ok(s) => emit(format!(
                "clip {n} ok frames={} fps={} loop={} bytes={} max_record={}",
                s.frames, s.fps, s.loop_to, s.bytes, s.max_record
            )),
            Err(e) => {
                bad += 1;
                emit(format!("clip {n} bad reason={}", quote(&e)));
            }
        }
    }
    if let Ok(list) = std::fs::read_to_string(dir.join("clips.txt")) {
        for l in list.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let name = l.split_whitespace().next().unwrap_or("");
            if !names.iter().any(|n| n == name) {
                bad += 1;
                emit(format!("list {name} bad reason=\"no such clip\""));
            }
        }
    }
    emit(format!("done clips={} bad={bad}", names.len()));
    if bad > 0 || names.is_empty() { EXIT_NO_CLIP } else { 0 }
}

fn convert(o: &Opts) -> Result<i32, (i32, String)> {
    let out = o.out.clone().ok_or((EXIT_USAGE, "convert needs --out".into()))?;
    if o.dirs.is_empty() {
        return Err((EXIT_USAGE, "convert needs --ps1 or --ps2".into()));
    }
    let listed = match &o.scenes {
        Some(p) => std::fs::read_to_string(p)
            .map_err(|e| format!("{}: {e}", p.display()))
            .and_then(|t| parse_scenes(&t))
            .map_err(|e| (EXIT_USAGE, e))?,
        None => Vec::new(),
    };
    let found = games(o);
    if found.is_empty() {
        return Err((EXIT_NO_GAME, "no game recognised".into()));
    }
    std::fs::create_dir_all(&out)
        .and_then(|_| std::fs::write(out.join(".write-test"), b""))
        .and_then(|_| std::fs::remove_file(out.join(".write-test")))
        .map_err(|e| (EXIT_OUT, format!("{}: {e}", out.display())))?;
    let work = std::env::temp_dir().join(format!("neochumby-pajamasam-{}", std::process::id()));

    // The scenes: the list's, for the games found, then the liveliest
    // rooms of each game's survey that the list doesn't have.
    let mut scenes: Vec<(&Game, PathBuf, Scene)> = Vec::new();
    for (g, d) in &found {
        for s in listed.iter().filter(|s| s.game == g.short) {
            scenes.push((g, d.clone(), s.clone()));
        }
        if o.auto > 0 {
            let have: Vec<u16> = listed.iter().filter(|s| s.game == g.short).map(|s| s.room).collect();
            for (room, name, _) in survey(g, d, &work).into_iter().filter(|r| !have.contains(&r.0)).take(o.auto) {
                scenes.push((
                    g,
                    d.clone(),
                    Scene {
                        name: format!("{}-room{room}", g.short),
                        game: g.short.into(),
                        room,
                        settle_ms: 2000,
                        seconds: o.seconds,
                        fps: o.fps,
                        title: format!("{} {name}", g.short.to_uppercase()),
                    },
                ));
            }
        }
    }

    // Recorded `jobs` at a time, each a ScummVM of its own.
    let results = std::sync::Mutex::new(Vec::new());
    let queue = std::sync::Mutex::new(scenes.into_iter());
    std::thread::scope(|s| {
        for _ in 0..o.jobs {
            s.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().next();
                    let Some((g, d, scene)) = next else { break };
                    let r = make_clip(g, &d, &scene, &work, &out, o.budget);
                    match &r {
                        Ok(line) => emit(line.clone()),
                        Err(e) => emit(format!("scene {} failed reason={}", scene.name, quote(e))),
                    }
                    results.lock().unwrap().push((scene, r.is_ok()));
                }
            });
        }
    });
    let _ = std::fs::remove_dir_all(&work);
    let results = results.into_inner().unwrap();
    let made: BTreeMap<String, (String, String)> = results
        .iter()
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| (s.name.clone(), (s.game.clone(), s.title.clone())))
        .collect();
    write_list(&out, &made).map_err(|e| (EXIT_OUT, format!("{}: {e}", out.display())))?;
    let bytes: u64 = made.keys().filter_map(|n| clip::read(&out, n).ok()).map(|r| r.stats.bytes).sum();
    let failed = results.len() - made.len();
    emit(format!("done clips={} failed={failed} bytes={bytes}", made.len()));
    Ok(if made.is_empty() { EXIT_NO_CLIP } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_parse() {
        let s = parse_scenes("# a comment\nps2-boat ps2 12 2000 16 12 The boat\n\nps1-x ps1 3 1000 10 10\n").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].room, s[0].title.as_str()), (12, "The boat"));
        assert!(parse_scenes("bad name! ps1 3 1000 10 10").is_err());
        assert!(parse_scenes("ps1-x ps1 three 1000 10 10").is_err());
    }

    #[test]
    fn rnam_both_layouts() {
        let mut wide = Vec::new();
        for (n, name) in [(1u16, "intro"), (12, "boat")] {
            wide.extend(n.to_le_bytes());
            wide.extend(name.as_bytes());
            wide.push(0);
        }
        wide.extend([0, 0]);
        assert_eq!(parse_rnam(&wide, true).unwrap(), [(1, "intro".into()), (12, "boat".into())]);
        let mut narrow = vec![3u8];
        let mut name = *b"castle\0\0\0";
        name.iter_mut().for_each(|c| *c ^= 0xFF);
        narrow.extend(name);
        narrow.push(0);
        assert_eq!(parse_rnam(&narrow, false).unwrap(), [(3, "castle".into())]);
    }

    /// Synthetic: a still picture with a moving block is lively; a still
    /// one, a blank one, or one that changes entirely each frame isn't.
    #[test]
    fn liveliness_ranks_rooms() {
        let still: Frame = (0..clip::PIXELS).map(|i| (i % 640) as u16 * 37).collect();
        let moving: Vec<Frame> = (0..8)
            .map(|t| {
                let mut f = still.clone();
                for y in 50..150 {
                    for x in (t * 20)..(t * 20 + 100) {
                        f[y * clip::W + x] = 0xFFFF;
                    }
                }
                f
            })
            .collect();
        assert!(liveliness(&moving).is_some());
        assert!(liveliness(&vec![still.clone(); 8]).is_none());
        assert!(liveliness(&vec![vec![0u16; clip::PIXELS]; 8]).is_none());
        let noise: Vec<Frame> = (0..8).map(|t| (0..clip::PIXELS).map(|i| (i * 7 + t * 13) as u16).collect()).collect();
        assert!(liveliness(&noise).is_none());
    }

    #[test]
    fn a_loop_within_budget() {
        // A 40-frame cycle recorded for 100 frames: the loop is found and
        // fits.
        let frames: Vec<Frame> = (0..100)
            .map(|t| {
                let mut f = vec![0x1234u16; clip::PIXELS];
                let x = (t % 40) * 6;
                for y in 0..60 {
                    f[y * clip::W + x..y * clip::W + x + 60].fill(0xF800);
                }
                f
            })
            .collect();
        let (c, d, secs) = loop_clip(frames, 12, 3 << 20);
        assert_eq!(d, 0.0);
        assert!(secs >= 3.0);
        assert_eq!(c.stats().frames % 40, 0);
        assert!(c.stats().bytes < 3 << 20);
    }
}
