//! The Chumby's devices, through the neochumby library: the screen
//! (/dev/fb0, written in place), the touchscreen (calibrated by
//! /psp/ts_settings), the top button and the speaker.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use neochumby::audio::{self, Speaker};
use neochumby::fb::{self, Fb, rgb};
use neochumby::input::{Touch, Touchscreen};

use crate::input::Raw;

/// The session (neochumby's proposal 0002) hands an app with `button =
/// true` the top button's open descriptor in this variable, because only
/// one process can have /dev/switch open.
const BUTTON_ENV: &str = "NEOCHUMBY_BUTTON";

pub struct Platform {
    fb: Option<Fb>,
    /// Drawn into when there's no /dev/fb0, so ScummVM still runs.
    spare: Vec<u16>,
    touch: Touchscreen,
    button: Option<File>,
}

impl Platform {
    pub fn open() -> Platform {
        let fb = Fb::open().map_err(|e| eprintln!("neochumby-scummvm: /dev/fb0: {e}")).ok();
        Platform { fb, spare: Vec::new(), touch: Touchscreen::open(), button: open_button() }
    }

    pub fn screen(&mut self) -> *mut u16 {
        match self.fb.as_mut() {
            Some(fb) => fb.pixels().as_mut_ptr(),
            None => {
                self.spare.resize(fb::W * fb::H, 0);
                self.spare.as_mut_ptr()
            }
        }
    }

    /// The screen is /dev/fb0 itself: what was drawn already shows.
    pub fn present(&mut self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    pub fn touches(&mut self, _now: u32, out: &mut Vec<Raw>) {
        out.extend(self.touch.poll().into_iter().map(|t| match t {
            Touch::Down { x, y, .. } => Raw::Down((x, y)),
            Touch::Up => Raw::Up,
        }));
    }

    /// The button's level. /dev/switch is level-triggered: every read
    /// returns the current state, so it is read once per poll.
    pub fn button(&mut self, _now: u32) -> bool {
        let mut b = [0u8; 1];
        matches!(self.button.as_mut().map(|f| f.read(&mut b)), Some(Ok(1))) && b[0] != 0
    }

    pub fn quit_requested(&mut self, _now: u32) -> bool {
        false
    }

    pub fn finish(&mut self) {}

    /// The game picker's screen.
    pub fn fb(&mut self) -> Option<&mut Fb> {
        self.fb.as_mut()
    }
}

/// The handed-over descriptor if there is one, else /dev/switch (free when
/// the session doesn't hold it), non-blocking either way.
fn open_button() -> Option<File> {
    if let Some(fd) = std::env::var(BUTTON_ENV).ok().and_then(|v| v.parse::<RawFd>().ok()) {
        // SAFETY: the session passes this descriptor to this process only,
        // open, for it to own; nothing else here uses or closes it. fcntl
        // only changes its flags.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0 {
                return Some(File::from_raw_fd(fd));
            }
        }
        eprintln!("neochumby-scummvm: {BUTTON_ENV}={fd} is not an open descriptor");
    }
    OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/switch").ok()
}

pub fn sleep_ms(ms: u32) {
    neochumby::sleep_ms(ms);
}

/// Where the audio goes: the speaker through neochumby's Speaker, which sets
/// the volume and unmutes while open and restores both when dropped. When
/// /dev/dsp is busy (another app or a service has it) the audio is dropped
/// at the same pace, and the speaker is tried again every few seconds.
pub struct Sink {
    speaker: Option<Speaker>,
    next_try: u32,
    bytes: Vec<u8>,
}

const RETRY_MS: u32 = 3000;

impl Sink {
    pub fn open(rate: u32) -> Sink {
        let mut s = Sink { speaker: None, next_try: 0, bytes: Vec::new() };
        s.try_open(rate);
        s
    }

    fn try_open(&mut self, rate: u32) {
        // The owner's volume, unless it's 0, as the stock boot can leave it.
        let volume = audio::volume().filter(|&v| v > 0).unwrap_or(70);
        match Speaker::open(2, rate, volume) {
            Ok(s) => self.speaker = Some(s),
            Err(e) => {
                eprintln!("neochumby-scummvm: the speaker: {e}; playing silently for now");
                self.next_try = crate::abi::millis().wrapping_add(RETRY_MS);
            }
        }
    }

    pub fn write(&mut self, pcm: &[i16], rate: u32) {
        if self.speaker.is_none() && (crate::abi::millis().wrapping_sub(self.next_try) as i32) >= 0 {
            self.try_open(rate);
        }
        if let Some(s) = self.speaker.as_mut() {
            self.bytes.clear();
            self.bytes.extend(pcm.iter().flat_map(|v| v.to_le_bytes()));
            if s.write_all(&self.bytes).is_ok() {
                return;
            }
            self.speaker = None;
            self.next_try = crate::abi::millis().wrapping_add(RETRY_MS);
        }
        sleep_ms(pcm.len() as u32 / 2 * 1000 / rate);
    }
}

/// Shows the games and returns the one tapped.
pub fn pick(games: &[String]) -> usize {
    let bg = rgb(16, 24, 48);
    let fg = rgb(255, 230, 140);
    let row = 40;
    let top = 40;
    crate::abi::with_platform(|p| {
        let Some(fb) = p.fb() else { return };
        fb.clear(bg);
        fb.text((fb::W - Fb::text_width("SCUMMVM", 2)) / 2, 12, "SCUMMVM", 2, fg);
        for (i, g) in games.iter().enumerate() {
            let y = top + i * row;
            fb.rect(16, y + 4, fb::W - 32, row - 8, rgb(40, 56, 96));
            let name: String = g.chars().take(16).collect::<String>().to_uppercase();
            fb.text(28, y + row / 2 - 8, &name, 2, fg);
        }
    });
    let mut out = Vec::new();
    let mut picked = None;
    loop {
        out.clear();
        crate::abi::with_platform(|p| p.touches(0, &mut out));
        for r in &out {
            match (*r, picked) {
                (Raw::Down((_, y)), None) if y as usize >= top && (y as usize - top) / row < games.len() => {
                    picked = Some((y as usize - top) / row);
                }
                // The game starts once the finger is lifted, so that the
                // tap isn't also its first click.
                (Raw::Up, Some(i)) => return i,
                _ => {}
            }
        }
        sleep_ms(20);
    }
}
