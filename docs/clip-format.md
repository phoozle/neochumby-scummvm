# The clip format (v1): looping animation for a 320x240 RGB565 screen

A clip is a short, silent animation that loops for as long as it shows, for
example a screensaver page. It is made offline, by any recorder, and played
by a light player that streams it from a stick or over NFS: one frame
record read ahead of the one showing, no decoding beyond copying pixels.
Nothing in it is specific to any game or source.

## Files

A clip `NAME` is two files, side by side:

| File | What |
| --- | --- |
| `NAME.idx` | The index: a 32-byte header, then one entry per record. About 1 KB for a 10-second loop |
| `NAME.clip` | The records, one after another: frame 0, frame 1, ..., frame N-1, then the wrap |

All integers are little-endian. Every record starts at an offset that is a
multiple of 4 and is a multiple of 4 bytes long, so a player can read a
record into an aligned buffer and use it in place (the ARM926 can't load
unaligned words).

## The index (`NAME.idx`)

| Offset | Type | Field | |
| --- | --- | --- | --- |
| 0 | 8 bytes | magic | `NCCLIP01` (ASCII) |
| 8 | u16 | version | 1 |
| 10 | u16 | width | 320 (the player may refuse anything else) |
| 12 | u16 | height | 240 |
| 14 | u16 | fps | frames a second, 1-50; 10-12 is usual |
| 16 | u32 | frames | N, at least 1 |
| 20 | u32 | loop | L, 0 <= L < N: the frame the loop goes back to |
| 24 | u32 | max_record | the largest record's size in bytes: one buffer of this size holds any record |
| 28 | u32 | data_size | the size of `NAME.clip` |
| 32 | N+1 entries | | each a u32 `offset` and a u32 `size`, of a record in `NAME.clip` |

Entries 0 to N-1 are the frames, in order. Entry N is the **wrap**: the
record that turns frame N-1's picture into frame L's. The index must be
consistent: entries in order, back to back from offset 0, the last ending
at `data_size`, each size at most `max_record`. A player should check that
and refuse the clip otherwise.

## A record

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u8 | kind: 0 = key (draws every pixel of the screen), 1 = delta (draws only what changed since the previous picture) |
| 1 | u8 | 0 |
| 2 | u16 | 0 |
| 4 | u32 | spans: how many follow |
| 8 | | the spans |

Each **span** is a run of pixels within one row:

| Type | Field |
| --- | --- |
| u32 | `start | len << 17 | fill << 31`: `start = y * width + x` (17 bits), `len` the pixel count, 1 to `width` and never past the row's end (9 bits), bits 26-30 zero, `fill` bit 31 |
| | if fill = 0: `len` RGB565 pixels (u16 each), then 2 bytes of padding if `len` is odd |
| | if fill = 1: one RGB565 pixel (u16), repeated `len` times, then 2 bytes of padding |

So a span is 4 bytes and a multiple of 4. Spans in a record are in screen
order (increasing `start`) and don't overlap. A record with no spans (8
bytes) is a frame where nothing changed.

Frame 0 is always a key: its spans cover every pixel once. The other frames
and the wrap are normally deltas, but any may be a key (a recorder may
write one where most of the picture changes, a scene cut).

RGB565 is the screen's own format (`0xF800` red, `0x07E0` green, `0x001F`
blue). There is no transparency: a clip owns the whole screen.

## Playing

```
read NAME.idx; check it
picture := frame 0 (a key)
show; i := 1
every 1/fps seconds:
    apply record i to the picture (copy each span's pixels, or fill)
    show the changed rows
    i := i + 1
    if i == N: apply the wrap; i := L + 1 (or L, if L == N - 1: then the wrap alone loops)
```

To be exact about the end: after frame N-1 is shown, the next picture is
the wrap applied to it, which is frame L's picture; the record after that
is frame L+1's. With L = N-1 and a one-frame clip (N = 1), the wrap
is empty and the picture stands still.

- **Streaming:** keep two buffers of `max_record` bytes; read record i+1
  (offset and size from the index) while record i shows, from a thread or
  between frames. On a Chumby's stick (about 870 KB/s) a record of 30 KB
  takes about 35 ms, so a clip meant for streaming should keep its delta
  records under about 40 KB at 12 fps (the recorder reports the largest).
- **Applying** a record is copying `len * 2` bytes per span into the
  framebuffer (or a fill), so the cost is proportional to what changed.
- **Late frames:** if a record isn't read in time, show the previous
  picture longer; never skip a record, as each depends on the last.
- **Starting over** is frame 0 again (a key), so a player can restart a
  clip at any time without the wrap.

## Sizes

A key is 240 spans of 320 pixels: 154,560 bytes. A delta costs 4 bytes per
span plus 2 per changed pixel. At 12 fps, a loop of 10 seconds whose deltas
average 20 KB is about 2.5 MB. A recorder should report each clip's size,
fps, frame count, loop length and largest record.

## Not in v1

No audio, no compression beyond spans and fills, no sizes other than the
screen's, no transparency, no seeking. A later version changes `version`
and may add fields after the header's 32 bytes, which v1 players reject.
