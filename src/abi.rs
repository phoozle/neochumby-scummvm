//! The C ABI the ScummVM backend calls (csrc/chumby.h), over the platform:
//! the Chumby's devices (device.rs) or the host test mode's stand-ins
//! (host.rs).
//!
//! ScummVM calls all of these from its main thread, except that the audio
//! thread started here calls back into it (`scummvm_chumby_mix`). The main
//! thread's state is behind a Mutex only it takes; the audio thread shares
//! nothing with it but an atomic stop flag. (std's Mutex must not be
//! contended on the Chumby: its futex flags are newer than the 2.6.16
//! kernel.)

use std::collections::VecDeque;
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::input::{self, Event};
use crate::platform::{self, Platform};

/// `struct nc_event` in csrc/chumby.h.
#[repr(C)]
pub struct NcEvent {
    kind: i32,
    x: i32,
    y: i32,
}

const NC_TOUCH_DOWN: i32 = 1;
const NC_TOUCH_UP: i32 = 2;
const NC_MENU: i32 = 3;
const NC_QUIT: i32 = 4;
const NC_ESCAPE: i32 = 5;

unsafe extern "C" {
    fn scummvm_chumby_mix(buf: *mut i16, frames: u32);
    fn scummvm_chumby_main(argc: c_int, argv: *mut *mut std::ffi::c_char) -> c_int;
}

/// Runs ScummVM with these arguments (the first is the program's name).
pub fn scummvm_main(args: &[String]) -> i32 {
    let mut argv: Vec<std::ffi::CString> =
        args.iter().map(|a| std::ffi::CString::new(a.as_str()).expect("an argument with a NUL")).collect();
    let mut ptrs: Vec<*mut std::ffi::c_char> = argv.iter_mut().map(|a| a.as_ptr() as *mut _).collect();
    ptrs.push(std::ptr::null_mut());
    // SAFETY: argv is a null-terminated array of NUL-terminated strings that
    // outlive the call, as main's are.
    unsafe { scummvm_chumby_main(args.len() as c_int, ptrs.as_mut_ptr()) }
}

struct State {
    platform: Platform,
    touch: input::Touch,
    button: input::Button,
    events: VecDeque<Event>,
    scratch: Vec<Event>,
    raw: Vec<input::Raw>,
    /// When the devices were last read (ms).
    read_at: Option<u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();
static SCREEN: AtomicPtr<u16> = AtomicPtr::new(std::ptr::null_mut());
/// Set by SIGTERM or SIGINT: the session ending the app. ScummVM is asked
/// to quit, so that the speaker is restored on the way out.
static TERM: AtomicBool = AtomicBool::new(false);

/// Reads the devices at most this often (ms): ScummVM polls in a loop
/// until there's nothing, and each reading is a system call per device.
const READ_EVERY_MS: u32 = 10;

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    let s = g.get_or_insert_with(|| State {
        platform: Platform::open(),
        touch: input::Touch::new(),
        button: input::Button::default(),
        events: VecDeque::new(),
        scratch: Vec::new(),
        raw: Vec::new(),
        read_at: None,
    });
    f(s)
}

/// Starts the clock and the termination handlers; call first.
pub fn init() {
    START.get_or_init(Instant::now);
    extern "C" fn on_term(_: c_int) {
        TERM.store(true, Ordering::SeqCst);
    }
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores to an atomic, which is
        // async-signal-safe.
        unsafe { libc::signal(sig, on_term as *const () as libc::sighandler_t) };
    }
}

pub fn millis() -> u32 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

/// Runs `f` with the platform, outside ScummVM (the game picker).
#[cfg(all(target_arch = "arm", target_os = "linux"))]
pub fn with_platform<R>(f: impl FnOnce(&mut Platform) -> R) -> R {
    with(|s| f(&mut s.platform))
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_screen() -> *mut u16 {
    let p = SCREEN.load(Ordering::Relaxed);
    if !p.is_null() {
        return p;
    }
    let p = with(|s| s.platform.screen());
    SCREEN.store(p, Ordering::Relaxed);
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_present(x: i32, y: i32, w: i32, h: i32) {
    with(|s| s.platform.present(x, y, w, h));
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_millis() -> u32 {
    millis()
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_sleep_ms(ms: u32) {
    platform::sleep_ms(ms);
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_poll(ev: *mut NcEvent) -> i32 {
    let next = with(|s| {
        let now = millis();
        if s.events.is_empty() && s.read_at.is_none_or(|t| now.wrapping_sub(t) >= READ_EVERY_MS) {
            s.read_at = Some(now);
            read(s, now);
        }
        s.events.pop_front()
    });
    let Some(e) = next else { return 0 };
    let (kind, (x, y)) = match e {
        Event::Down(p) => (NC_TOUCH_DOWN, p),
        Event::Up(p) => (NC_TOUCH_UP, p),
        Event::Escape => (NC_ESCAPE, (0, 0)),
        Event::Menu => (NC_MENU, (0, 0)),
        Event::Quit => (NC_QUIT, (0, 0)),
    };
    // SAFETY: ScummVM passes a pointer to its own nc_event.
    unsafe { *ev = NcEvent { kind, x, y } };
    1
}

fn read(s: &mut State, now: u32) {
    s.raw.clear();
    s.platform.touches(now, &mut s.raw);
    for r in s.raw.drain(..) {
        s.touch.feed(now, r, &mut s.scratch);
    }
    s.touch.tick(now, &mut s.scratch);
    let pressed = s.platform.button(now);
    s.button.feed(now, pressed, &mut s.scratch);
    if TERM.swap(false, Ordering::SeqCst) || s.platform.quit_requested(now) {
        s.scratch.push(Event::Quit);
    }
    s.events.extend(s.scratch.drain(..));
}

struct Audio {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

static AUDIO: Mutex<Option<Audio>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn nc_audio_start(rate: u32, frames: u32) -> u32 {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let mut sink = platform::Sink::open(rate);
            let mut buf = vec![0i16; frames as usize * 2];
            while !flag.load(Ordering::Relaxed) {
                // SAFETY: buf holds `frames` stereo frames.
                unsafe { scummvm_chumby_mix(buf.as_mut_ptr(), frames) };
                // Paced by the sink: a speaker blocks while its buffer is
                // full, and the stand-ins sleep for the buffer's length.
                sink.write(&buf, rate);
            }
        })
        .expect("the audio thread");
    *AUDIO.lock().unwrap() = Some(Audio { stop, thread });
    rate
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_audio_stop() {
    if let Some(a) = AUDIO.lock().unwrap().take() {
        a.stop.store(true, Ordering::Relaxed);
        let _ = a.thread.join();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nc_exit(code: i32) {
    finish();
    std::process::exit(code);
}

/// Stops the audio (the speaker's volume and mute restored) and lets the
/// platform finish (the host test mode writes its files).
pub fn finish() {
    nc_audio_stop();
    with(|s| s.platform.finish());
}
