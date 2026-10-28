//! The touchscreen and the top button, turned into what ScummVM is given
//! (`nc_event`s): a pure state machine over timed readings, so it runs the
//! same on the Chumby and in the host test mode, and is unit tested.
//!
//! The Chumby's resistive touchscreen has two habits (neochumby's
//! docs/hardware.md): a steady press reports brief releases, and a tap can
//! be followed by an echo press shortly after it ends. Both would be extra
//! clicks in a game. So a release is only passed on once the finger has
//! stayed up for [`RELEASE_MS`] (a press before then continues the first),
//! and a press within [`ECHO_MS`] of a release, near where it ended, is
//! dropped with everything until its own release.

/// Screen pixels, 320x240.
pub type Point = (i32, i32);

/// A reading from the touchscreen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raw {
    /// Finger down (or still down) at a point.
    Down(Point),
    Up,
}

/// What ScummVM gets (`enum nc_event_type` in csrc/chumby.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Down(Point),
    Up(Point),
    /// The Escape key: skips a cutscene.
    Escape,
    /// ScummVM's menu (save, load, quit).
    Menu,
    Quit,
}

/// How long the finger must stay up for a release to count.
pub const RELEASE_MS: u32 = 120;
/// How soon after a release a press near it is taken for an echo.
pub const ECHO_MS: u32 = 350;
/// How near (in screen pixels, each way) an echo is to the release.
pub const ECHO_PX: i32 = 16;
/// Holding the top button this long opens ScummVM's menu; a shorter press
/// is Escape.
pub const HOLD_MENU_MS: u32 = 800;
/// Holding the top button this long quits.
pub const HOLD_QUIT_MS: u32 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TouchState {
    Idle,
    Pressed(Point),
    /// Lifted at a time, at a point: not yet a release.
    Lifting(u32, Point),
    /// An echo, swallowed until it lifts.
    Echo,
}

pub struct Touch {
    state: TouchState,
    /// The last release passed on: when the finger lifted, and where.
    released: Option<(u32, Point)>,
}

impl Default for Touch {
    fn default() -> Touch {
        Touch::new()
    }
}

impl Touch {
    pub fn new() -> Touch {
        Touch { state: TouchState::Idle, released: None }
    }

    /// One reading at time `now` (ms).
    pub fn feed(&mut self, now: u32, raw: Raw, out: &mut Vec<Event>) {
        self.tick(now, out);
        self.state = match (self.state, raw) {
            (TouchState::Idle, Raw::Down(p)) => {
                let echo = self.released.is_some_and(|(t, q)| {
                    now.wrapping_sub(t) < ECHO_MS && (p.0 - q.0).abs() <= ECHO_PX && (p.1 - q.1).abs() <= ECHO_PX
                });
                if echo {
                    TouchState::Echo
                } else {
                    out.push(Event::Down(p));
                    TouchState::Pressed(p)
                }
            }
            (TouchState::Idle, Raw::Up) => TouchState::Idle,
            (TouchState::Pressed(q) | TouchState::Lifting(_, q), Raw::Down(p)) => {
                if p != q {
                    out.push(Event::Down(p));
                }
                TouchState::Pressed(p)
            }
            (TouchState::Pressed(p), Raw::Up) => TouchState::Lifting(now, p),
            (s @ TouchState::Lifting(..), Raw::Up) => s,
            (TouchState::Echo, Raw::Down(_)) => TouchState::Echo,
            (TouchState::Echo, Raw::Up) => TouchState::Idle,
        };
    }

    /// Passes on a release once the finger has been up long enough.
    pub fn tick(&mut self, now: u32, out: &mut Vec<Event>) {
        if let TouchState::Lifting(t, p) = self.state
            && now.wrapping_sub(t) >= RELEASE_MS
        {
            out.push(Event::Up(p));
            self.released = Some((t, p));
            self.state = TouchState::Idle;
        }
    }
}

/// The top button (the squeeze switch). A press is Escape, which skips a
/// cutscene, when it is let go; held for [`HOLD_MENU_MS`] it opens
/// ScummVM's menu (save, load, quit) instead, and held on to
/// [`HOLD_QUIT_MS`] it quits.
#[derive(Default)]
pub struct Button {
    since: Option<u32>,
    menu_sent: bool,
    quit_sent: bool,
}

impl Button {
    /// The button's level at time `now`.
    pub fn feed(&mut self, now: u32, pressed: bool, out: &mut Vec<Event>) {
        match (self.since, pressed) {
            (None, true) => {
                self.since = Some(now);
                self.menu_sent = false;
                self.quit_sent = false;
            }
            (Some(t), true) => {
                let held = now.wrapping_sub(t);
                if !self.menu_sent && held >= HOLD_MENU_MS {
                    out.push(Event::Menu);
                    self.menu_sent = true;
                }
                if !self.quit_sent && held >= HOLD_QUIT_MS {
                    out.push(Event::Quit);
                    self.quit_sent = true;
                }
            }
            (Some(_), false) => {
                if !self.menu_sent {
                    out.push(Event::Escape);
                }
                self.since = None;
            }
            (None, false) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(readings: &[(u32, Raw)], until: u32) -> Vec<Event> {
        let mut t = Touch::new();
        let mut out = Vec::new();
        for &(at, r) in readings {
            t.feed(at, r, &mut out);
        }
        t.tick(until, &mut out);
        out
    }

    #[test]
    fn tap() {
        let out = run(&[(0, Raw::Down((10, 20))), (80, Raw::Up)], 1000);
        assert_eq!(out, [Event::Down((10, 20)), Event::Up((10, 20))]);
    }

    #[test]
    fn release_waits() {
        let out = run(&[(0, Raw::Down((10, 20))), (80, Raw::Up)], 80 + RELEASE_MS - 1);
        assert_eq!(out, [Event::Down((10, 20))]);
    }

    #[test]
    fn brief_release_in_a_drag_is_ignored() {
        let out = run(
            &[(0, Raw::Down((10, 20))), (50, Raw::Down((30, 20))), (60, Raw::Up), (90, Raw::Down((40, 20)))],
            90 + RELEASE_MS - 1,
        );
        assert_eq!(out, [Event::Down((10, 20)), Event::Down((30, 20)), Event::Down((40, 20))]);
    }

    #[test]
    fn echo_is_dropped() {
        let out = run(&[(0, Raw::Down((10, 20))), (80, Raw::Up), (300, Raw::Down((12, 22))), (350, Raw::Up)], 2000);
        assert_eq!(out, [Event::Down((10, 20)), Event::Up((10, 20))]);
    }

    #[test]
    fn a_tap_elsewhere_is_not_an_echo() {
        let out = run(&[(0, Raw::Down((10, 20))), (80, Raw::Up), (300, Raw::Down((200, 120))), (350, Raw::Up)], 2000);
        assert_eq!(out, [Event::Down((10, 20)), Event::Up((10, 20)), Event::Down((200, 120)), Event::Up((200, 120))]);
    }

    #[test]
    fn a_later_tap_in_place_is_not_an_echo() {
        let out = run(&[(0, Raw::Down((10, 20))), (80, Raw::Up), (1000, Raw::Down((10, 20))), (1080, Raw::Up)], 2000);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn button_press_escape_hold_menu_then_quit() {
        let mut b = Button::default();
        let mut out = Vec::new();
        b.feed(0, true, &mut out);
        b.feed(300, false, &mut out);
        assert_eq!(out, [Event::Escape]);
        out.clear();
        b.feed(1000, true, &mut out);
        b.feed(1000 + HOLD_MENU_MS, true, &mut out);
        b.feed(1000 + HOLD_MENU_MS + 100, false, &mut out);
        assert_eq!(out, [Event::Menu]);
        out.clear();
        b.feed(5000, true, &mut out);
        b.feed(5000 + HOLD_MENU_MS, true, &mut out);
        b.feed(5000 + HOLD_QUIT_MS, true, &mut out);
        b.feed(5000 + HOLD_QUIT_MS + 500, true, &mut out);
        b.feed(5000 + HOLD_QUIT_MS + 900, false, &mut out);
        assert_eq!(out, [Event::Menu, Event::Quit]);
    }
}
