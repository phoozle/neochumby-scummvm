//! The host test mode (any build not for the Chumby): the same backend and
//! scaler, with stand-ins for the devices, to check the port on a desktop
//! with no window.
//!
//! - The screen is a 320x240 RGB565 buffer, saved as PNG when the script
//!   says (`shot`), into NEOCHUMBY_SCUMMVM_OUT (default `out`).
//! - Touches and the top button come from a script, NEOCHUMBY_SCUMMVM_SCRIPT:
//!   one step a line, `MS ACTION ARGS`, MS counted from the start, in screen
//!   pixels (320x240):
//!
//!   ```text
//!   # comment
//!   5000 tap 160 120        a tap (down, up 80 ms later)
//!   6000 down 10 10         finger down, moved (down again), lifted
//!   6100 down 40 10
//!   6200 up
//!   7000 drag 10 10 200 10 500   down, moved over 500 ms, up
//!   7500 esc                a short press of the top button (Escape)
//!   8000 menu               the top button held until ScummVM's menu opens
//!   9000 hold               the top button held until it quits
//!   9500 shot title         saves OUT/title.png
//!   9900 quit               the session's SIGTERM
//!   ```
//!
//! - The audio goes to NEOCHUMBY_SCUMMVM_WAV, a 16-bit stereo WAV, if set,
//!   at the pace a speaker would take it.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::input::Raw;

const W: usize = 320;
const H: usize = 240;

enum Step {
    Touch(Raw),
    Button(bool),
    Shot(String),
    Quit,
}

pub struct Platform {
    screen: Vec<u16>,
    steps: VecDeque<(u32, Step)>,
    out: PathBuf,
    button: bool,
    touching: bool,
    quit: bool,
    presents: u64,
    pixels: u64,
}

impl Platform {
    pub fn open() -> Platform {
        let out = PathBuf::from(std::env::var_os("NEOCHUMBY_SCUMMVM_OUT").unwrap_or_else(|| "out".into()));
        let steps = match std::env::var_os("NEOCHUMBY_SCUMMVM_SCRIPT") {
            Some(path) => {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("NEOCHUMBY_SCUMMVM_SCRIPT {}: {e}", path.to_string_lossy()));
                parse(&text)
            }
            None => VecDeque::new(),
        };
        if steps.iter().any(|(_, s)| matches!(s, Step::Shot(_))) {
            std::fs::create_dir_all(&out).expect("NEOCHUMBY_SCUMMVM_OUT");
        }
        Platform {
            screen: vec![0; W * H],
            steps,
            out,
            button: false,
            touching: false,
            quit: false,
            presents: 0,
            pixels: 0,
        }
    }

    pub fn screen(&mut self) -> *mut u16 {
        self.screen.as_mut_ptr()
    }

    pub fn present(&mut self, _x: i32, _y: i32, w: i32, h: i32) {
        self.presents += 1;
        self.pixels += (w * h) as u64;
        self.run(crate::abi::millis(), None);
    }

    pub fn touches(&mut self, now: u32, out: &mut Vec<Raw>) {
        self.run(now, Some(out));
    }

    pub fn button(&mut self, _now: u32) -> bool {
        self.button
    }

    /// The script's `quit`, once.
    pub fn quit_requested(&mut self, _now: u32) -> bool {
        std::mem::take(&mut self.quit)
    }

    pub fn finish(&mut self) {
        eprintln!(
            "neochumby-scummvm (host): {} screen updates, {:.1} full screens of pixels",
            self.presents,
            self.pixels as f64 / (W * H) as f64
        );
    }

    /// Runs the steps that are due. Touches wait for the next poll (`out`),
    /// so that each is read in order, as from the touchscreen.
    fn run(&mut self, now: u32, mut out: Option<&mut Vec<Raw>>) {
        while let Some((at, _)) = self.steps.front() {
            if (now.wrapping_sub(*at) as i32) < 0 {
                break;
            }
            if matches!(self.steps[0].1, Step::Touch(_)) && out.is_none() {
                break;
            }
            let (at, step) = self.steps.pop_front().unwrap();
            match step {
                Step::Touch(r) => {
                    match r {
                        Raw::Down(p) if !self.touching => {
                            eprintln!("neochumby-scummvm (host): {at} ms: touch at {p:?}")
                        }
                        Raw::Up => eprintln!("neochumby-scummvm (host): {at} ms: lifted"),
                        _ => {}
                    }
                    self.touching = matches!(r, Raw::Down(_));
                    out.as_mut().unwrap().push(r);
                }
                Step::Button(b) => self.button = b,
                Step::Shot(name) => self.shot(&name),
                Step::Quit => self.quit = true,
            }
        }
    }

    fn shot(&self, name: &str) {
        let path = self.out.join(format!("{name}.png"));
        let mut rgb = Vec::with_capacity(W * H * 3);
        for &p in &self.screen {
            let (r, g, b) = ((p >> 11) & 31, (p >> 5) & 63, p & 31);
            rgb.extend([(r << 3 | r >> 2) as u8, (g << 2 | g >> 4) as u8, (b << 3 | b >> 2) as u8]);
        }
        match std::fs::write(&path, png(W as u32, H as u32, &rgb)) {
            Ok(()) => eprintln!("neochumby-scummvm (host): {} ms: saved {}", crate::abi::millis(), path.display()),
            Err(e) => eprintln!("neochumby-scummvm (host): {}: {e}", path.display()),
        }
    }
}

fn parse(text: &str) -> VecDeque<(u32, Step)> {
    let mut steps: Vec<(u32, Step)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> i32 {
            f.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("script line {}: {line}", n + 1))
        };
        let at = num(0) as u32;
        match f.get(1).copied() {
            Some("tap") => {
                steps.push((at, Step::Touch(Raw::Down((num(2), num(3))))));
                steps.push((at + 80, Step::Touch(Raw::Up)));
            }
            Some("down") => steps.push((at, Step::Touch(Raw::Down((num(2), num(3)))))),
            Some("up") => steps.push((at, Step::Touch(Raw::Up))),
            Some("drag") => {
                let (x0, y0, x1, y1, ms) = (num(2), num(3), num(4), num(5), num(6).max(20));
                let n = ms / 20;
                for i in 0..=n {
                    let p = (x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n);
                    steps.push((at + (i * 20) as u32, Step::Touch(Raw::Down(p))));
                }
                steps.push((at + ms as u32 + 20, Step::Touch(Raw::Up)));
            }
            Some("esc") => {
                steps.push((at, Step::Button(true)));
                steps.push((at + 200, Step::Button(false)));
            }
            Some("menu") => {
                steps.push((at, Step::Button(true)));
                steps.push((at + crate::input::HOLD_MENU_MS + 200, Step::Button(false)));
            }
            Some("hold") => {
                steps.push((at, Step::Button(true)));
                steps.push((at + crate::input::HOLD_QUIT_MS + 500, Step::Button(false)));
            }
            Some("shot") => steps.push((at, Step::Shot(f.get(2).unwrap_or(&"shot").to_string()))),
            Some("quit") => steps.push((at, Step::Quit)),
            _ => panic!("script line {}: {line}", n + 1),
        }
    }
    // Stable: steps at the same time keep their order.
    steps.sort_by_key(|(t, _)| *t);
    steps.into()
}

pub fn sleep_ms(ms: u32) {
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}

/// The audio, to a WAV file if NEOCHUMBY_SCUMMVM_WAV is set, at a
/// speaker's pace either way.
pub struct Sink {
    wav: Option<File>,
    bytes: u32,
    start: std::time::Instant,
    frames: u64,
}

impl Sink {
    pub fn open(rate: u32) -> Sink {
        let wav = std::env::var_os("NEOCHUMBY_SCUMMVM_WAV").and_then(|p| {
            if let Some(dir) = std::path::Path::new(&p).parent().filter(|d| !d.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut f = File::create(&p).map_err(|e| eprintln!("NEOCHUMBY_SCUMMVM_WAV: {e}")).ok()?;
            f.write_all(&wav_header(rate, 0)).ok()?;
            Some(f)
        });
        Sink { wav, bytes: 0, start: std::time::Instant::now(), frames: 0 }
    }

    pub fn write(&mut self, pcm: &[i16], rate: u32) {
        if let Some(f) = self.wav.as_mut() {
            let bytes: Vec<u8> = pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
            if f.write_all(&bytes).is_ok() {
                self.bytes += bytes.len() as u32;
                // Kept valid as it grows, in case the run is killed.
                let _ = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&wav_header(rate, self.bytes)));
                let _ = f.seek(SeekFrom::End(0));
            }
        }
        // A real-time pace: as long as the audio written so far lasts.
        self.frames += pcm.len() as u64 / 2;
        let due = self.frames * 1000 / rate as u64;
        let elapsed = self.start.elapsed().as_millis() as u64;
        if due > elapsed {
            sleep_ms((due - elapsed) as u32);
        }
    }
}

fn wav_header(rate: u32, data: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend(b"RIFF");
    h.extend((36 + data).to_le_bytes());
    h.extend(b"WAVEfmt ");
    h.extend(16u32.to_le_bytes());
    h.extend(1u16.to_le_bytes()); // PCM
    h.extend(2u16.to_le_bytes()); // stereo
    h.extend(rate.to_le_bytes());
    h.extend((rate * 4).to_le_bytes());
    h.extend(4u16.to_le_bytes());
    h.extend(16u16.to_le_bytes());
    h.extend(b"data");
    h.extend(data.to_le_bytes());
    h
}

/// A PNG of 8-bit RGB pixels, uncompressed (deflate's stored blocks): no
/// dependencies, and only for test screenshots.
fn png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    // Each line starts with filter 0 (none).
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for line in rgb.chunks(w as usize * 3) {
        raw.push(0);
        raw.extend(line);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend(*b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// No picker on the host: NEOCHUMBY_SCUMMVM_GAME names the game, else the
/// first.
pub fn pick(games: &[String]) -> usize {
    std::env::var("NEOCHUMBY_SCUMMVM_GAME").ok().and_then(|g| games.iter().position(|x| *x == g)).unwrap_or(0)
}
