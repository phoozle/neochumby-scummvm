//! Clips (docs/clip-format.md): looping 320x240 RGB565 animation, a key
//! frame then deltas of spans, with an index for streaming. Writing (the
//! encoder and the loop finder), reading and playing back (to check a clip,
//! and for tests). Nothing here knows where the frames came from.

use std::io;
use std::path::Path;

pub const W: usize = 320;
pub const H: usize = 240;
pub const PIXELS: usize = W * H;
const MAGIC: &[u8; 8] = b"NCCLIP01";
const HEADER: usize = 32;
const KIND_KEY: u8 = 0;
const KIND_DELTA: u8 = 1;
const FILL: u32 = 1 << 31;

/// A frame: PIXELS RGB565 pixels, row-major.
pub type Frame = Vec<u16>;

/// Unchanged pixels between two changed runs of a row, at most this many,
/// are sent anyway rather than starting a new span: a span header is 4
/// bytes, 2 pixels' worth.
const MERGE_GAP: usize = 2;
/// A run of one colour at least this long becomes a fill span.
const MIN_FILL: usize = 6;

/// One record: a key when `prev` is None, else the changes from `prev`.
pub fn encode_record(prev: Option<&[u16]>, cur: &[u16]) -> Vec<u8> {
    assert_eq!(cur.len(), PIXELS);
    let mut spans: Vec<(usize, usize)> = Vec::new(); // (start, len), within rows
    for y in 0..H {
        let row = y * W;
        let changed = |x: usize| prev.is_none_or(|p| p[row + x] != cur[row + x]);
        let mut x = 0;
        while x < W {
            if !changed(x) {
                x += 1;
                continue;
            }
            let start = x;
            let mut end = x + 1; // exclusive, last changed + 1
            let mut probe = end;
            while probe < W {
                if changed(probe) {
                    end = probe + 1;
                    probe += 1;
                } else if probe - end < MERGE_GAP {
                    probe += 1;
                } else {
                    break;
                }
            }
            spans.push((row + start, end - start));
            x = end;
        }
    }
    let mut out = Vec::with_capacity(8 + spans.len() * 8);
    out.extend([if prev.is_none() { KIND_KEY } else { KIND_DELTA }, 0, 0, 0]);
    out.extend([0u8; 4]); // the span count, filled in below
    let mut count = 0u32;
    for (start, len) in spans {
        // Split each span into runs of one colour (fills) and the rest.
        let px = &cur[start..start + len];
        let mut i = 0;
        let mut lit = 0; // where the pending literal run started
        while i < len {
            let mut j = i + 1;
            while j < len && px[j] == px[i] {
                j += 1;
            }
            if j - i >= MIN_FILL {
                if lit < i {
                    push_span(&mut out, start + lit, &px[lit..i]);
                    count += 1;
                }
                out.extend(((start + i) as u32 | ((j - i) as u32) << 17 | FILL).to_le_bytes());
                out.extend(px[i].to_le_bytes());
                out.extend([0, 0]);
                count += 1;
                lit = j;
            }
            i = j;
        }
        if lit < len {
            push_span(&mut out, start + lit, &px[lit..]);
            count += 1;
        }
    }
    out[4..8].copy_from_slice(&count.to_le_bytes());
    out
}

fn push_span(out: &mut Vec<u8>, start: usize, px: &[u16]) {
    out.extend((start as u32 | (px.len() as u32) << 17).to_le_bytes());
    for p in px {
        out.extend(p.to_le_bytes());
    }
    if !px.len().is_multiple_of(2) {
        out.extend([0, 0]);
    }
}

/// Applies a record to a picture. Errors on anything the spec forbids.
pub fn apply(record: &[u8], picture: &mut [u16]) -> Result<(), String> {
    if record.len() < 8 || !record.len().is_multiple_of(4) {
        return Err("record: too short or not a multiple of 4".into());
    }
    if record[0] > KIND_DELTA || record[1..4] != [0, 0, 0] {
        return Err("record: bad kind".into());
    }
    let count = u32::from_le_bytes(record[4..8].try_into().unwrap());
    let mut p = 8;
    let mut last_end = 0usize;
    let mut covered = 0usize;
    for _ in 0..count {
        let word = u32::from_le_bytes(record.get(p..p + 4).ok_or("record: span past the end")?.try_into().unwrap());
        p += 4;
        let start = (word & 0x1ffff) as usize;
        let len = ((word >> 17) & 0x1ff) as usize;
        if word & 0x7c00_0000 != 0 || len == 0 || len > W || start % W + len > W || start + len > PIXELS {
            return Err(format!("record: bad span {word:#x}"));
        }
        if start < last_end {
            return Err("record: spans out of order or overlapping".into());
        }
        last_end = start + len;
        covered += len;
        if word & FILL != 0 {
            let b = record.get(p..p + 4).ok_or("record: fill past the end")?;
            picture[start..start + len].fill(u16::from_le_bytes([b[0], b[1]]));
            p += 4;
        } else {
            let bytes = (len * 2).next_multiple_of(4);
            let b = record.get(p..p + bytes).ok_or("record: pixels past the end")?;
            for (i, px) in b[..len * 2].as_chunks::<2>().0.iter().enumerate() {
                picture[start + i] = u16::from_le_bytes([px[0], px[1]]);
            }
            p += bytes;
        }
    }
    if p != record.len() {
        return Err("record: bytes after the last span".into());
    }
    if record[0] == KIND_KEY && covered != PIXELS {
        return Err("record: a key that doesn't cover the screen".into());
    }
    Ok(())
}

/// A clip as written: its records (frames, then the wrap) and header.
pub struct Clip {
    pub fps: u16,
    pub loop_to: u32,
    pub records: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub frames: u32,
    pub fps: u16,
    pub loop_to: u32,
    pub bytes: u64,
    pub max_record: u32,
}

impl Clip {
    /// Encodes `frames`, looping back to frame `loop_to` after the last.
    /// A delta that would be bigger than a key is written as a key.
    pub fn encode(frames: &[Frame], fps: u16, loop_to: u32) -> Clip {
        assert!(!frames.is_empty() && (loop_to as usize) < frames.len());
        let mut records = Vec::with_capacity(frames.len() + 1);
        for (i, f) in frames.iter().enumerate() {
            records.push(best(if i == 0 { None } else { Some(&frames[i - 1]) }, f));
        }
        records.push(best(Some(frames.last().unwrap()), &frames[loop_to as usize]));
        Clip { fps, loop_to, records }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            frames: (self.records.len() - 1) as u32,
            fps: self.fps,
            loop_to: self.loop_to,
            bytes: self.records.iter().map(|r| r.len() as u64).sum::<u64>() + self.index().len() as u64,
            max_record: self.records.iter().map(|r| r.len() as u32).max().unwrap_or(0),
        }
    }

    pub fn index(&self) -> Vec<u8> {
        let data: u64 = self.records.iter().map(|r| r.len() as u64).sum();
        let mut idx = Vec::with_capacity(HEADER + self.records.len() * 8);
        idx.extend(MAGIC);
        idx.extend(1u16.to_le_bytes());
        idx.extend((W as u16).to_le_bytes());
        idx.extend((H as u16).to_le_bytes());
        idx.extend(self.fps.to_le_bytes());
        idx.extend(((self.records.len() - 1) as u32).to_le_bytes());
        idx.extend(self.loop_to.to_le_bytes());
        idx.extend(self.stats_max().to_le_bytes());
        idx.extend((data as u32).to_le_bytes());
        let mut off = 0u32;
        for r in &self.records {
            idx.extend(off.to_le_bytes());
            idx.extend((r.len() as u32).to_le_bytes());
            off += r.len() as u32;
        }
        idx
    }

    fn stats_max(&self) -> u32 {
        self.records.iter().map(|r| r.len() as u32).max().unwrap_or(0)
    }

    /// Writes `BASE.idx` and `BASE.clip`, each through a `.tmp` and a
    /// rename, the data first, so a reader never sees an index without its
    /// data.
    pub fn write(&self, dir: &Path, name: &str) -> io::Result<()> {
        let data: Vec<u8> = self.records.concat();
        let clip = dir.join(format!("{name}.clip"));
        let idx = dir.join(format!("{name}.idx"));
        let tmp = |p: &Path| p.with_extension(format!("{}.tmp", p.extension().unwrap().to_string_lossy()));
        std::fs::write(tmp(&clip), &data)?;
        std::fs::write(tmp(&idx), self.index())?;
        std::fs::rename(tmp(&clip), &clip)?;
        std::fs::rename(tmp(&idx), &idx)
    }
}

/// A delta, unless a key is smaller.
fn best(prev: Option<&[u16]>, cur: &[u16]) -> Vec<u8> {
    let key = || encode_record(None, cur);
    match prev {
        None => key(),
        Some(p) => {
            let d = encode_record(Some(p), cur);
            if d.len() > PIXELS * 2 {
                let k = key();
                if k.len() < d.len() {
                    return k;
                }
            }
            d
        }
    }
}

/// A clip read back: its header and records, checked against the spec.
pub struct Read {
    pub stats: Stats,
    pub records: Vec<Vec<u8>>,
}

pub fn read(dir: &Path, name: &str) -> Result<Read, String> {
    let idx = std::fs::read(dir.join(format!("{name}.idx"))).map_err(|e| format!("{name}.idx: {e}"))?;
    let data = std::fs::read(dir.join(format!("{name}.clip"))).map_err(|e| format!("{name}.clip: {e}"))?;
    if idx.len() < HEADER || &idx[..8] != MAGIC {
        return Err(format!("{name}.idx: not a clip index"));
    }
    let u16_at = |o: usize| u16::from_le_bytes([idx[o], idx[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(idx[o..o + 4].try_into().unwrap());
    let (version, w, h, fps) = (u16_at(8), u16_at(10), u16_at(12), u16_at(14));
    let (frames, loop_to, max_record, data_size) = (u32_at(16), u32_at(20), u32_at(24), u32_at(28));
    if version != 1 || w as usize != W || h as usize != H || !(1..=50).contains(&fps) {
        return Err(format!("{name}.idx: version {version}, {w}x{h}, {fps} fps"));
    }
    if frames == 0 || loop_to >= frames || idx.len() != HEADER + (frames as usize + 1) * 8 {
        return Err(format!("{name}.idx: {frames} frames, loop {loop_to}, {} bytes", idx.len()));
    }
    if data_size as usize != data.len() {
        return Err(format!("{name}.clip: {} bytes, the index says {data_size}", data.len()));
    }
    let mut records = Vec::new();
    let mut next = 0u32;
    for i in 0..=frames as usize {
        let (off, size) = (u32_at(HEADER + i * 8), u32_at(HEADER + i * 8 + 4));
        if off != next || size > max_record || !off.is_multiple_of(4) || !size.is_multiple_of(4) {
            return Err(format!("{name}.idx: entry {i} ({off}, {size})"));
        }
        next = off + size;
        records.push(data[off as usize..next as usize].to_vec());
    }
    if next != data_size {
        return Err(format!("{name}.idx: the entries end at {next}, not {data_size}"));
    }
    if records[0][0] != KIND_KEY {
        return Err(format!("{name}: frame 0 isn't a key"));
    }
    let stats = Stats { frames, fps, loop_to, bytes: (idx.len() + data.len()) as u64, max_record };
    Ok(Read { stats, records })
}

impl Read {
    /// Plays the clip once round, and its wrap: the pictures of frames 0
    /// to N-1, then frame L's again from the wrap.
    pub fn play(&self) -> Result<Vec<Frame>, String> {
        let mut picture = vec![0u16; PIXELS];
        let mut out = Vec::new();
        for (i, r) in self.records.iter().enumerate() {
            apply(r, &mut picture).map_err(|e| format!("record {i}: {e}"))?;
            out.push(picture.clone());
        }
        Ok(out)
    }
}

/// How unlike two frames are: the mean difference per pixel of the sum of
/// the three components' differences (each scaled to 0-255), on every
/// fourth pixel of every fourth row.
pub fn distance(a: &[u16], b: &[u16]) -> f64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in (0..H).step_by(4) {
        for x in (0..W).step_by(4) {
            let (p, q) = (a[y * W + x], b[y * W + x]);
            let d = |s: u32, m: u32, k: u32| {
                (((p as u32 >> s) & m) as i32 - ((q as u32 >> s) & m) as i32).unsigned_abs() * k
            };
            sum += (d(11, 31, 8) + d(5, 63, 4) + d(0, 31, 8)) as u64;
            n += 1;
        }
    }
    sum as f64 / n as f64
}

/// The share of pixels that differ between two frames.
pub fn changed(a: &[u16], b: &[u16]) -> f64 {
    a.iter().zip(b).filter(|(p, q)| p != q).count() as f64 / PIXELS as f64
}

/// Where to cut a recording into a loop: frames `a..b`, then back to `a`,
/// with frame `b` (not in the clip) the most like frame `a`. Looks at
/// starts in the first third and lengths from `min_len` frames. Returns
/// (a, b, the distance between frames a and b).
pub fn find_loop(frames: &[Frame], min_len: usize) -> Option<(usize, usize, f64)> {
    let n = frames.len();
    let mut best: Option<(usize, usize, f64)> = None;
    for a in 0..(n / 3).max(1) {
        for b in (a + min_len.max(2))..n {
            let d = distance(&frames[a], &frames[b]);
            // Longer loops win ties: they repeat less obviously.
            if best.is_none_or(|(ba, bb, bd)| d < bd - 0.05 || (d <= bd + 0.05 && b - a > bb - ba)) {
                best = Some((a, b, d));
            }
        }
    }
    best
}

/// Blends the last `k` frames of `frames[a..b]` towards the frames before
/// `a` (which the loop's start follows on from), so that the end runs into
/// the start without a jump. Needs `a >= k`.
pub fn crossfade(frames: &mut [Frame], a: usize, b: usize, k: usize) {
    assert!(a >= k && b - a > k);
    for i in 0..k {
        // Weight of the frame before the loop's start, 1/(k+1) .. k/(k+1).
        let w = (i + 1) as u32 * 256 / (k as u32 + 1);
        let src = frames[a - k + i].clone();
        let dst = &mut frames[b - k + i];
        for (d, s) in dst.iter_mut().zip(&src) {
            *d = blend(*d, *s, w);
        }
    }
}

/// `a` blended with `b` at weight `w` of 256, per RGB565 component.
pub fn blend(a: u16, b: u16, w: u32) -> u16 {
    let mix = |s: u32, m: u32| ((((a as u32 >> s) & m) * (256 - w) + ((b as u32 >> s) & m) * w) >> 8) << s;
    (mix(11, 31) | mix(5, 63) | mix(0, 31)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic scene: a still background (bands of colour), a block
    /// that moves round a circuit of `period` frames, and a patch that
    /// flickers between two colours. Nothing from any game.
    fn scene(t: usize, period: usize) -> Frame {
        let mut f = vec![0u16; PIXELS];
        for y in 0..H {
            for x in 0..W {
                f[y * W + x] = ((y / 30) as u16 * 0x1082) ^ ((x / 40) as u16 * 0x0821);
            }
        }
        let phase = t % period;
        let bx = 20 + phase * 200 / period;
        for y in 100..140 {
            for x in bx..bx + 40 {
                f[y * W + x] = 0xF800 | (x as u16 & 0x1f);
            }
        }
        let c = if t.is_multiple_of(2) { 0x07E0 } else { 0x001F };
        for y in 10..20 {
            f[y * W + 300..y * W + 310].fill(c);
        }
        f
    }

    #[test]
    fn a_key_round_trips() {
        let f = scene(3, 24);
        let r = encode_record(None, &f);
        let mut p = vec![0xFFFFu16; PIXELS];
        apply(&r, &mut p).unwrap();
        assert_eq!(p, f);
        assert_eq!(r.len() % 4, 0);
    }

    #[test]
    fn deltas_are_small_and_exact() {
        let (a, b) = (scene(3, 24), scene(4, 24));
        let r = encode_record(Some(&a), &b);
        let mut p = a.clone();
        apply(&r, &mut p).unwrap();
        assert_eq!(p, b);
        assert!(r.len() < 20_000, "{} bytes", r.len());
        assert_eq!(encode_record(Some(&a), &a).len(), 8);
    }

    #[test]
    fn odd_and_edge_spans() {
        let a = vec![0u16; PIXELS];
        let mut b = a.clone();
        b[0] = 1; // one pixel at the very start
        b[W - 1] = 2; // the end of the first row
        b[PIXELS - 3..].fill(7); // a short run at the very end
        b[5 * W..6 * W].fill(9); // a whole row of one colour: a fill
        let r = encode_record(Some(&a), &b);
        let mut p = a.clone();
        apply(&r, &mut p).unwrap();
        assert_eq!(p, b);
    }

    #[test]
    fn a_clip_writes_reads_and_loops() {
        let period = 24;
        let frames: Vec<Frame> = (0..period).map(|t| scene(t, period)).collect();
        let clip = Clip::encode(&frames, 12, 0);
        let dir = std::env::temp_dir().join(format!("nc-clip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        clip.write(&dir, "synthetic").unwrap();
        let back = read(&dir, "synthetic").unwrap();
        assert_eq!(back.stats, clip.stats());
        let pictures = back.play().unwrap();
        assert_eq!(&pictures[..period], &frames[..]);
        assert_eq!(pictures[period], frames[0], "the wrap goes back to frame 0");
        assert!(back.stats.bytes < 200_000 + 24 * 20_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_loop_into_the_middle() {
        let frames: Vec<Frame> = (0..10).map(|t| scene(t, 10)).collect();
        let clip = Clip::encode(&frames, 10, 4);
        let back = Read { stats: clip.stats(), records: clip.records.clone() };
        let pictures = back.play().unwrap();
        assert_eq!(pictures[10], frames[4]);
    }

    #[test]
    fn the_loop_is_found() {
        // Three and a bit circuits of a 20-frame cycle.
        let frames: Vec<Frame> = (0..70).map(|t| scene(t + 3, 20)).collect();
        let (a, b, d) = find_loop(&frames, 15).unwrap();
        assert_eq!(d, 0.0);
        assert_eq!((b - a) % 20, 0);
    }

    #[test]
    fn crossfade_ends_near_the_start() {
        let mut frames: Vec<Frame> = (0..40).map(|t| scene(t * 7, 33)).collect();
        let before = distance(&frames[37], &frames[3]);
        crossfade(&mut frames, 4, 38, 4);
        assert!(distance(&frames[37], &frames[3]) < before);
    }

    #[test]
    fn corrupt_records_are_refused() {
        let a = vec![0u16; PIXELS];
        let mut r = encode_record(None, &a);
        r[8] ^= 0xff; // the first span's start
        r[10] = 0xff; // and its length
        assert!(apply(&r, &mut vec![0; PIXELS]).is_err());
        assert!(apply(&[1, 0, 0, 0, 1, 0, 0, 0], &mut vec![0; PIXELS]).is_err());
    }
}
