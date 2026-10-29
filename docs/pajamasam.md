# neochumby-pajamasam: screensaver clips from your own Pajama Sam games

`neochumby-pajamasam` turns **your own copies** of Pajama Sam (1 and 2)
into looping, silent clips ([clip-format.md](clip-format.md)) for a
Chumby's screensaver. It runs on the server (Linux x86_64 or macOS), never
on the Chumby, and writes only to the directory you give it. Nothing of the
games is in this repository, and the clips it makes are yours alone: don't
publish them.

How it works: it runs ScummVM headless (the app's host test mode, with the
same 2:1 scaler as the Chumby, and no cursor), skips the intro, enters a
room through ScummVM's debugger (`room N`), lets it idle with no input, and
records it at the clip's fps. It finds where the picture comes back to
where the loop starts and cuts there (crossfading the end into the start if
it never quite does), halves the fps or shortens the loop to fit the
budget, and writes the clip.

Rooms come from [scenes.txt](../scenes.txt) and, with `--auto N`, from a
survey: every room is entered for a few seconds and ranked by how much of
the picture keeps moving (3-60% of the pixels changing, steadily; not
blank, not a full-screen change), and the N liveliest are recorded.

## Commands

```
neochumby-pajamasam convert --out DIR [--ps1 DIR] [--ps2 DIR] [--scenes FILE] [--auto N]
                            [--fps N] [--seconds N] [--budget-mb N] [--jobs N]
neochumby-pajamasam survey (--ps1 DIR | --ps2 DIR)    each room's liveliness, ranked
neochumby-pajamasam check [--ps1 DIR] [--ps2 DIR]     are these the games?
neochumby-pajamasam rooms (--ps1 DIR | --ps2 DIR)     room numbers and names (from the index)
neochumby-pajamasam verify DIR                        are these clips sound?
neochumby-pajamasam encode REC.raw --out DIR          a host-mode recording to a clip
```

Defaults: `--auto 4` (per game), `--fps 12`, `--seconds 16` (recorded; the
loop is cut from it), `--budget-mb 3` (per clip), `--jobs` the CPUs.

**The game folders.** `--ps1`: `Pajama.HE0`, `Pajama.(a)` (or `.HE1`),
`.HE2`, `.HE4`. `--ps2`: `PAJAMA2.HE0`, `.HE1`, `.HE2`, `.HE4`. Any case;
other files are ignored.

**What it writes**, in `--out`: `NAME.idx` and `NAME.clip` for each clip
(each through a `.tmp` and a rename), then `clips.txt`, atomically: one line
a clip, `NAME GAME FPS FRAMES SECONDS BYTES TITLE...`, every clip in the
directory that reads back. A re-run replaces clips of the same name and
leaves the rest.

## Progress (stdout)

One event a line, an event word, then `key=value` fields (values with
spaces double-quoted). stderr is ScummVM's log, for people.

```
game ps1 found path=/srv/games/ps1
game ps2 missing reason="no PAJAMA2.HE0"
survey ps2 room=12 of=80 name=boat lively=0.084
scene ps2-room12 recording title="PS2 boat"
scene ps2-room12 done frames=120 fps=12 loop=0 seconds=10.0 bytes=2104320 max_record=38912 loop_error=0.60
scene ps1-room30 failed reason="ScummVM exited 1"
done clips=6 failed=1 bytes=13456789
```

## Exit codes

| Code | |
| --- | --- |
| 0 | At least one clip written (see `failed` lines for the rest) |
| 1 | Usage |
| 2 | No game recognised in the folders given |
| 3 | Games recognised, but no clip made (`verify`: a bad clip, or none) |
| 4 | `--out` can't be created or written |

## Building

```sh
cargo build --release --bin neochumby-pajamasam      # this machine (Linux or macOS)
cargo build --release --target x86_64-unknown-linux-musl --bin neochumby-pajamasam   # static, zig on PATH
```

It needs no display or audio device. About a minute a clip, plus a few
minutes a game for `--auto`'s survey; one core and about 50 MB a job.

## Tests

`cargo test` checks the clip encoder, the loop finder, the scene list and
the room-name parser on synthetic frames and data only.
