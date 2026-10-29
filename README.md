# neochumby-scummvm

Humongous Entertainment's **Pajama Sam** games on the original Chumby
(the Chumby Classic), as a full-screen [neochumby](https://github.com/johnmerchant/neochumby)
app: [ScummVM](https://www.scummvm.org) 2026.4.0's SCUMM engine with its HE
support, on a small backend for the Chumby's screen, touchscreen, top
button and speaker.

**Status:** it builds for the Chumby, and in its host test mode it plays
Pajama Sam 2 and Pajama Sam 1 from start to the first rooms: the intro
skipped, the inventory opened by touch, a game saved through the game's own
save screen, ScummVM's menu opened and closed. **It hasn't run on a Chumby
yet.** See [Performance](#performance) for what to expect there.

No game data is in this repository, and none may be added: you bring your
own copies of the games.

## Your games

Each game goes in a directory of its own beside the app's binary, in the
neochumby tree's `apps/scummvm/`. Any directory name works; ScummVM
detects the game from its files. Copy these from the game's CD:

| Game | Directory (suggested) | Files |
| --- | --- | --- |
| Pajama Sam 2: Thunder and Lightning Aren't So Frightening | `apps/scummvm/pajama2/` | `PAJAMA2.HE0`, `PAJAMA2.HE1`, `PAJAMA2.HE2`, `PAJAMA2.HE4` (about 200 MB) |
| Pajama Sam: No Need to Hide When It's Dark Outside | `apps/scummvm/pajama/` | `Pajama.HE0`, `Pajama.(a)`, `Pajama.HE2`, `Pajama.HE4` (about 225 MB) |

The `.HE0` file is the game's index, `.HE1` or `.(a)` its rooms and
scripts, `.HE2` its speech and sounds, `.HE4` its music. Nothing else on
the CD is read (the `.EXE`s, `.W32`, `.HLP`, `CATALOG/`). The names are
matched without regard to case. Other SCUMM and HE games that ScummVM
supports should work the same way (`apps/scummvm/<game>/` with the files
[ScummVM's wiki](https://wiki.scummvm.org/index.php/Datafiles) lists), but
only these two have been tried.

With one game, the app starts it. With more than one, it shows a list to
tap (five fit on the screen).

## Installing into a neochumby tree

The app installs like any other neochumby app from git (neochumby's
`tools/tree`, from a neochumby checkout):

```sh
tools/tree OUT -d DATA git+git@github.com:phoozle/neochumby-scummvm.git#REV
```

where `DATA` is a tree of your own game files laid out as the tree is:

```
DATA/apps/scummvm/pajama2/PAJAMA2.HE0
DATA/apps/scummvm/pajama2/PAJAMA2.HE1
DATA/apps/scummvm/pajama2/PAJAMA2.HE2
DATA/apps/scummvm/pajama2/PAJAMA2.HE4
```

`tools/stick /Volumes/STICK -d DATA git+...#REV` writes the same to a stick.
The repository is private, so the URL is one your git can clone (SSH above);
a local checkout's path works too (`tools/tree OUT -d DATA ../neochumby-scummvm`).

**Which neochumby:** the manifest says `button = true` (the app has the top
button) and `interrupt = false` (no service replaces it in front). Those
keys come with neochumby's session PRs #15-#17 (proposal 0002); until they
merge, run `tools/tree` from a checkout of `feat/session-button`
(`7a908ef`, as CI does). The binary itself works with either: it takes the
button from the session (`NEOCHUMBY_BUTTON`) when given it, else opens
`/dev/switch`.

## Playing

| | |
| --- | --- |
| Tap | Click (the cursor moves there, then the button goes down) |
| Drag | Move with the button down. Drag to the bottom edge to show the inventory |
| Top button, pressed | Escape: skips the current cutscene |
| Top button, held 0.8 seconds | ScummVM's menu (options, about, quit) |
| Top button, held 3 seconds | Quit |
| Top-left corner, held 3 seconds | neochumby's own way out: ends the app, whatever it is doing |

The HE games save and load through their own screens, not ScummVM's menu
(which says so): in Pajama Sam, drag to the bottom of the screen, tap the
computer, then SAVE or LOAD, and tap a slot. No name needs typing.

A tap and the click it makes are a little apart in time: the cursor moves
first and the button goes down 50 ms later, so the game has seen the
cursor over what was touched. A release only counts once the finger has
been up for 120 ms, and a press within 350 ms of a release, near it, is
taken for the touchscreen's echo and dropped (neochumby's
`docs/hardware.md` describes both habits).

**Saves** go to the first of these that can be written:

1. `NEOCHUMBY_SCUMMVM_SAVES`, if set;
2. `apps/scummvm/saves/`, when the tree is writable (a stick mounted
   read-write): the saves stay with the games, which is where they belong;
3. `/psp/scummvm/`, the Chumby's own flash, which keeps them across
   reboots. It is only 2 MB, shared with the Chumby's settings, and a
   Pajama Sam save is about 200 KB, so saves go there only while 1 MB stays
   free;
4. `/tmp/scummvm-saves/` (RAM): lost when the Chumby is switched off.

ScummVM's own settings file is `$HOME/scummvm-chumby.ini` (`/tmp`). The
app passes everything it needs on ScummVM's command line, so nothing there
has to survive.

## How it's built

```
src/main.rs           finds the games, picks one, runs ScummVM
src/abi.rs            the C ABI the backend calls (csrc/chumby.h)
src/input.rs          touches and the top button, as ScummVM gets them (unit tested)
src/device.rs         the Chumby: neochumby's fb, touchscreen, Speaker
src/host.rs           the host test mode's stand-ins
csrc/chumby-backend.cpp    ScummVM's OSystem: events, mixer, clock, managers
csrc/chumby-graphics.cpp   the game's screen scaled to 320x240, the overlay, the cursor
csrc/config/          what ScummVM's configure generates, for both builds
csrc/scummvm-sources.txt   the ScummVM files compiled
third_party/scummvm/  ScummVM 2026.4.0, the files this app compiles
tools/vendor-scummvm  regenerates third_party/scummvm and csrc/config
```

**The backend is C++ inside ScummVM's backend structure, over a thin C
ABI to a Rust platform layer, as neochumby's Doom is.** ScummVM's `OSystem`
is a C++ class hierarchy, so the backend's ScummVM side has to be C++; what
it needs from the device is small (a screen pointer, events, a clock, an
audio thread). Keeping the devices in Rust means they go through
neochumby's own library, which already knows the Chumby's traps: the
touchscreen's calibration in `/psp/ts_settings`, the speaker's mute and
volume (`neochumby::audio::Speaker` sets and restores both), `poll()` for
sleeping (a restarted `nanosleep` fails with `EINVAL` on its kernel), and
crash reports. The same ABI has a second implementation for the host test
mode, so the C++ is exactly what runs on the Chumby.

**ScummVM** is built with only the SCUMM engine and HE (`--disable-all-engines
--enable-engine=scumm,he`, without the v7/v8 games), statically, and with
everything optional off: no MT-32 or FluidSynth, networking, cloud, text to
speech, OpenGL, SDL, scalers, translations or external codec libraries
(`tools/scummvm-configure-options`). HE needs the Bink decoder, which is
ScummVM's own. ScummVM's configure and make aren't used by the build:
`tools/vendor-scummvm` ran them once, for the host and for the Chumby, and
kept the list of files they compiled and the `config.h` they generated;
`build.rs` compiles that list with the `cc` crate.

**For the Chumby** the compiler is zig's C++ (`tools/zigcxx`, zig 0.17,
`arm-linux-musleabi`, `-mcpu=arm926ej_s`, soft float), the binary links
zig's libc++ statically, and the Rust standard library supplies the
`__sync_*` atomics ARMv5 has no instructions for (through the kernel's
helpers). Nothing in it uses the FPU's instructions.

**The screen.** The HE games draw at 640x480 in 256 colours. The backend
keeps that picture and, on each update, halves the parts that changed into
the Chumby's 320x240 RGB565 framebuffer, each output pixel the average of a
2x2 block (not every other pixel): each palette entry is kept "spread" in
32 bits, so a block is four table lookups, three additions, a shift and a
mask, two pixels to a 32-bit store, all integer. 320x200 games are shown
1:1, centred. The cursor is halved the same way when the game is, and drawn
on the scaled picture. ScummVM's own menus (the overlay) are drawn at
320x240 directly. The screen mode is never changed (`docs/hardware.md`:
it breaks the LCD controller) and there is no page flipping (the Chumby has
none), so the picture is written to `/dev/fb0` in place, as neochumby's
Doom does.

**Audio** is ScummVM's mixer at 22050 Hz stereo, the HE games' own rate,
filled 1024 frames (46 ms) at a time by an audio thread and written to
`/dev/dsp` through `neochumby::audio::Speaker`, which unmutes the speaker
and sets the volume while open and puts both back when closed. If
`/dev/dsp` is busy, the game plays silently, at the same pace (the HE
scripts wait for their sounds to end), and the speaker is tried again every
3 seconds. On SIGTERM (the session ending the app) ScummVM is asked to
quit, so that the speaker is restored on the way out.

## Building

The host test mode, on macOS or Linux (any C++ compiler):

```sh
cargo build --release
cargo test --release
```

For the Chumby: Rust with the `armv5te-unknown-linux-musleabi` target and
zig 0.17 on `PATH`, as for neochumby (`.cargo/config.toml` sets the linker):

```sh
rustup target add armv5te-unknown-linux-musleabi
cargo build --release --target armv5te-unknown-linux-musleabi
```

ScummVM takes about 20 seconds to compile on 12 cores.

Without rustup (Homebrew's Rust has no prebuilt standard library for the
Chumby, but ships its source), the standard library can be built along
with the app; this is how it was built and checked on a Mac:

```sh
RUSTC_BOOTSTRAP=1 cargo build --release --target armv5te-unknown-linux-musleabi -Zbuild-std=std,panic_abort
```

The same works for neochumby's `tools/tree`, with
`RUSTC_BOOTSTRAP=1 CARGO_UNSTABLE_BUILD_STD=std,panic_abort` in the
environment.

`neochumby-scummvm --scummvm ARGS...` runs ScummVM with `ARGS` as they are
(`--scummvm --version`, `--scummvm --list-engines`), and
`NEOCHUMBY_SCUMMVM_ARGS` adds options to the normal command line
(`--debugflags=SCRIPTS -d2`).

## The host test mode

Built for anything but the Chumby, the app runs with no window: the
screen is a 320x240 RGB565 buffer saved as PNG when a script says so,
touches and the top button come from the script, and the audio goes to a
WAV file. The backend, the scaler and the input handling are the same code
as on the Chumby.

```sh
NEOCHUMBY_SCUMMVM_GAMES=~/games \       # a directory holding pajama2/ (your files)
NEOCHUMBY_SCUMMVM_SCRIPT=intro.txt \
NEOCHUMBY_SCUMMVM_OUT=shots \           # PNGs go here (default: out)
NEOCHUMBY_SCUMMVM_WAV=audio.wav \       # optional
NEOCHUMBY_SCUMMVM_SAVES=/tmp/saves \
NEOCHUMBY_SCUMMVM_GAME=pajama2 \        # with several games: which (else the first)
target/release/neochumby-scummvm
```

The script is one step a line, `MS ACTION ARGS`, MS counted from the start
and positions in screen pixels (320x240):

```
3000 esc                 # a short press of the top button: skips the intro
6000 esc
9000 esc
12000 drag 160 120 160 238 400   # down, moved over 400 ms, up: the inventory
13500 tap 16 218         # the computer
16000 shot panel         # saves OUT/panel.png
20000 menu               # the top button held: ScummVM's menu
22000 tap 160 170        # Resume
24000 quit               # as the session's SIGTERM
```

Also `down X Y` (down, or moved while down), `up`, and `hold` (the top
button held until it quits). The screenshots and audio are made from your
game: keep them to yourself (`.gitignore` leaves out `*.png` and `*.wav`).

## Performance

The Chumby's ARM926 runs at about 266-350 MHz with no FPU, roughly a third
of a GP2X's budget, and ScummVM's HE engine works on a 640x480 picture.
Measured in the host test mode (Apple Silicon, one core), ScummVM used
0.45 s of CPU for the first 32 s of Pajama Sam 2's intro and 0.36 s for
40 s ending in the first room, updating the equivalent of 9-10 full
screens a second. An ARM926 at 300 MHz is something like 60-100 times
slower per core than that, so **expect the Chumby to be near or past its
limit in busy scenes**: the full-screen cutscenes are the worst, the rooms
much lighter. Until it is measured on a device, that is an estimate, not a
number. To measure: run with `NEOCHUMBY_PROF=file` and read the profile with
neochumby's `tools/profile file neochumby-scummvm`.

Where the time should go, and what's been done about it:

- **The 2:1 scaler** (`scaleRows` in `csrc/chumby-graphics.cpp`): per output
  pixel four byte loads, four table loads (a 1 KB table, in the 16 KB data
  cache), integer adds and one 32-bit store per two pixels; only changed
  rectangles are scaled. A full screen is 76,800 output pixels; at ~10 full
  screens a second this should be a few per cent of the CPU.
- **The engine's own drawing at 640x480** (HE's `wiz` image decoding and
  compositing, `drawStripToScreen`): likely the largest share. ScummVM has
  ARM assembly for `drawStripToScreen` (`engines/scumm/gfxARM.s`,
  `USE_ARM_GFX_ASM`) which isn't enabled yet: it assumes word-aligned
  buffers and hasn't been checked with the HE games.
- **The mixer**: ScummVM mixes in integer (fixed-point rate conversion), at
  22050 Hz, so no resampling for the HE games' sounds. There is no float in
  the mixer or the scaler; anything ScummVM does in float elsewhere goes
  through neochumby's ARM soft float.
- **Reading the game** from a stick: about 870 KB/s (`docs/hardware.md`),
  and ScummVM reads resources from the main thread, so room changes will
  pause.

## Known limits

- **Not yet run on a Chumby.** Nothing here has met the 2.6.16 kernel, the
  real touchscreen, `/dev/dsp` or the top button handover.
- **Speed** (above). If it's too slow, the next steps are the ARM assembly
  above, a profile, and a cheaper path for full-screen updates.
- **Audio latency** is whatever the OSS driver buffers: `Speaker` doesn't
  set the fragment size. If speech and lips drift apart, neochumby's
  `Speaker` needs a way to set `SNDCTL_DSP_SETFRAGMENT`.
- **No keyboard.** Escape is on the top button; nothing sends Enter,
  Space (the HE games' pause) or letters. Pajama Sam's save screen doesn't
  need a name.
- **Hover.** A finger moves the cursor only while touching, so things that
  react to the cursor passing over them react when touched. The inventory
  is reached by dragging to the bottom edge.
- **Horizontal screen shakes** aren't drawn (vertical ones are).
- **The top-left corner** is neochumby's way out: a 3-second hold there
  ends the app, so don't rest a finger on it.
- **ScummVM's launcher** isn't used: with several games the app shows its
  own list (up to five). ScummVM's menu has "Return to Launcher", which
  would show ScummVM's launcher at 320x240; that hasn't been tried.
- The tree's directory for each game is fixed at start; adding games means
  adding directories.

## Updating ScummVM

```sh
git clone --depth 1 --branch v2026.4.0 https://github.com/scummvm/scummvm /tmp/scummvm
tools/vendor-scummvm /tmp/scummvm
```

It checks the checkout is at `csrc/SCUMMVM_VERSION`, runs ScummVM's
configure and make for the host and (with zig) for the Chumby, and rewrites
`third_party/scummvm/` (only the files compiled and the files they
include, with ScummVM's licences), `csrc/scummvm-sources.txt` and
`csrc/config/`. For a new release, change `csrc/SCUMMVM_VERSION` first. No
file in `third_party/scummvm` is changed from upstream.

## The screensaver converter

`neochumby-pajamasam` makes looping screensaver clips of Pajama Sam 1 and 2
from **your own** game files, on your server: see
[docs/pajamasam.md](docs/pajamasam.md) and the clip format,
[docs/clip-format.md](docs/clip-format.md). It records with the host test
mode above; the clips it makes are made from the games, so like the games
they stay yours: never committed here, never published.

## Licence

ScummVM is GPL-3.0-or-later, and this app links it, so the whole crate,
our Rust and C++ included, is **GPL-3.0-or-later** (`LICENSE`). ScummVM's
own notices are in `third_party/scummvm/` (`COPYING`, `COPYRIGHT`,
`AUTHORS`, `LICENSES/`). `tools/zigcc` is a copy of neochumby's (MIT).
Pajama Sam and the other games are Humongous Entertainment's; none of
their data is here.
