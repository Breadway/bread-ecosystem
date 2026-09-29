//! A small baseline + progressive (Huffman) JPEG decoder whose output is
//! bit-identical to libjpeg-turbo's default decode, which is what
//! ImageMagick (and therefore pywal) sees. The general-purpose decoder in
//! `image` (zune-jpeg) uses a different IDCT, chroma upsampler and colour
//! conversion, which moves decoded samples by a few levels and is enough to
//! flip an octree bucket now and then (see [`crate::extract`]).
//!
//! Ported from libjpeg-turbo 3.x with its default decompression parameters
//! (`JDCT_ISLOW`, `do_fancy_upsampling`, no scaling):
//!
//! * `jidctint.c::jpeg_idct_islow`, including its zero-column/zero-row
//!   shortcuts and the post-IDCT range-limit table (`jdmaster.c`).
//! * `jdsample.c`: `h2v1`/`h2v2` fancy (triangle) upsampling, `h1v2` fancy,
//!   plain replication otherwise, with `jdmainct.c`'s edge handling
//!   (first/last sample row duplicated for context).
//! * `jdcolor.c::ycc_rgb_convert` with its fixed-point tables.
//! * `jdapimin.c`'s colour space guess (JFIF, Adobe transform, component
//!   ids).
//!
//! Anything outside that (arithmetic coding, 12-bit or lossless JPEGs,
//! CMYK/YCCK, progressive files whose low-frequency coefficients were never
//! fully sent, where libjpeg-turbo applies block smoothing) returns `None`
//! so the caller falls back to the `image` crate.
//!
//! Everything ported here (`jidctint.c`, `jdsample.c`, `jdcolor.c`,
//! `jdapimin.c`, `jdmainct.c`/`jdmaster.c`, Huffman decode) belongs to
//! libjpeg-turbo's core libjpeg API library, which is IJG-License-only; the
//! Modified (3-clause) BSD License covers the separate TurboJPEG API,
//! cjpeg/djpeg and the build/test system, none of which is used here. See
//! `NOTICE` and `LICENSE-libjpeg-turbo` at the root of this crate.

/// `jpeg_natural_order`: zig-zag index -> natural (row-major) index.
const NATURAL: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Upper bound on the total bytes this decoder will allocate for one
/// image's coefficient blocks, per-component planes and final interleaved
/// output combined. The SOF marker's width/height/sampling factors are
/// attacker-controlled input (read before a single byte of entropy-coded
/// scan data is checked), so every buffer this decoder sizes from them
/// must be checked against a budget first, or a tiny file with a huge
/// declared size can force a multi-gigabyte allocation before decoding
/// ever gets far enough to fail on its own (a 22 KB JPEG declaring
/// 65500x65500 measured at 11.4 GB resident before the OOM killer step in).
///
/// 2 GiB comfortably covers a real 16K (15360x8640) wallpaper decoded at
/// worst-case 4:4:4 chroma sampling (a bit over 1 GiB across coefficients,
/// planes and output), while still rejecting headers many times larger
/// than any real photo or wallpaper format would declare. [`crate::extract`]
/// uses the same figure as an explicit `image::Limits::max_alloc` for the
/// png/webp/gif/bmp path, so every format this crate decodes is held to one
/// documented ceiling instead of the `image` crate's undocumented 512 MiB
/// default.
pub(crate) const MAX_DECODE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A decoded image: interleaved 8-bit samples, 1 (gray) or 3 (RGB)
/// channels.
pub(crate) struct Decoded {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<u8>,
}

#[derive(Clone, Default)]
struct Huffman {
    /// (code length, code) -> value, as `maxcode`/`valptr` tables.
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], values: Vec<u8>) -> Option<Self> {
        let mut h = Huffman {
            maxcode: [-1; 18],
            valptr: [0; 17],
            mincode: [0; 17],
            values,
        };
        let mut code = 0i32;
        let mut k = 0i32;
        for len in 1..=16 {
            let n = counts[len - 1] as i32;
            if n > 0 {
                h.valptr[len] = k;
                h.mincode[len] = code;
                code += n;
                k += n;
                h.maxcode[len] = code - 1;
            } else {
                h.maxcode[len] = -1;
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        if k as usize > h.values.len() {
            return None;
        }
        Some(h)
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per row/column in the coefficient buffer (MCU padded).
    bw: usize,
    bh: usize,
    /// Blocks actually covering the component (non-interleaved scans).
    real_bw: usize,
    real_bh: usize,
    /// `downsampled_width` / `downsampled_height`.
    dw: usize,
    dh: usize,
    coefs: Vec<[i16; 64]>,
    dc_tbl: usize,
    ac_tbl: usize,
    /// `coef_bits` for the first ten coefficients (progressive only):
    /// -1 = never sent, otherwise the last successive-approximation Al.
    coef_bits: [i32; 10],
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    nbits: u32,
    /// A marker was hit inside entropy-coded data: feed zeros from here on.
    hit_marker: bool,
    /// The buffer ran out entirely (as opposed to `hit_marker`, which is a
    /// legitimate marker such as EOI found within bounds): the file is
    /// truncated. libjpeg-turbo would synthesize a fake EOI and keep going
    /// with a warning; this decoder does not replicate that recovery path,
    /// so a truncated file must not silently produce wrong-but-plausible
    /// colours. Checked once the scan finishes; see [`decode`].
    truncated: bool,
}

impl<'a> Reader<'a> {
    fn fill(&mut self) {
        while self.nbits <= 56 {
            let byte = if self.hit_marker || self.pos >= self.data.len() {
                if !self.hit_marker && self.pos >= self.data.len() {
                    self.truncated = true;
                }
                0
            } else {
                let b = self.data[self.pos];
                if b == 0xFF {
                    let next = self.data.get(self.pos + 1).copied().unwrap_or(0);
                    if next == 0x00 {
                        self.pos += 2;
                        0xFF
                    } else {
                        self.hit_marker = true;
                        0
                    }
                } else {
                    self.pos += 1;
                    b
                }
            };
            self.bits |= (byte as u64) << (56 - self.nbits);
            self.nbits += 8;
        }
    }

    fn get_bits(&mut self, n: u32) -> i32 {
        if n == 0 {
            return 0;
        }
        self.fill();
        let v = (self.bits >> (64 - n)) as i32;
        self.bits <<= n;
        self.nbits -= n;
        v
    }

    fn get_bit(&mut self) -> bool {
        self.get_bits(1) != 0
    }

    fn decode(&mut self, h: &Huffman) -> u8 {
        self.fill();
        let mut code = 0i32;
        for len in 1..=16 {
            code = (code << 1) | ((self.bits >> 63) as i32);
            self.bits <<= 1;
            self.nbits -= 1;
            if code <= h.maxcode[len] {
                let idx = (h.valptr[len] + code - h.mincode[len]) as usize;
                return h.values.get(idx).copied().unwrap_or(0);
            }
        }
        // Corrupt data: libjpeg-turbo warns and returns 0.
        0
    }

    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.get_bits(s);
        if v < (1 << (s - 1)) {
            v + ((-1i32) << s) + 1
        } else {
            v
        }
    }

    /// Skip to (and past) the next RSTn marker, resetting the bit buffer.
    fn restart(&mut self) {
        self.bits = 0;
        self.nbits = 0;
        self.hit_marker = false;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                return;
            }
            if self.data[self.pos] == 0xFF
                && self.data[self.pos + 1] != 0
                && self.data[self.pos + 1] != 0xFF
            {
                // Some other marker: leave it for the caller.
                return;
            }
            self.pos += 1;
        }
    }

    /// Position just after this scan's entropy-coded data.
    fn end_of_scan(&self) -> usize {
        let mut p = self.pos;
        while p + 1 < self.data.len() {
            if self.data[p] == 0xFF {
                let n = self.data[p + 1];
                if n != 0 && !(0xD0..=0xD7).contains(&n) && n != 0xFF {
                    return p;
                }
                p += 2;
            } else {
                p += 1;
            }
        }
        self.data.len()
    }
}

fn be16(d: &[u8], p: usize) -> Option<usize> {
    Some(((*d.get(p)? as usize) << 8) | *d.get(p + 1)? as usize)
}

/// Decode `data` exactly like libjpeg-turbo, or `None` when the file uses a
/// feature this decoder does not replicate (see the module docs) or is
/// malformed. `*truncated_out` is set when `None` came specifically from
/// the entropy-coded data running out mid-scan (a genuinely truncated
/// file), and `*over_budget_out` is set when `None` came specifically from
/// the SOF header's declared dimensions exceeding [`MAX_DECODE_BUDGET_BYTES`]
/// before anything was allocated for them. Both are distinct from every
/// other reason to bail (an unsupported feature within budget):
/// [`crate::extract::Source::open`] uses `truncated_out` to fail outright on
/// a truncated file, and `over_budget_out` to fail outright on an
/// over-budget one, instead of letting the generic `image`-crate fallback
/// guess at its own recovery or, worse, decode the same oversized image a
/// second time with a different, unbounded allocator.
pub(crate) fn decode(
    data: &[u8],
    truncated_out: &mut bool,
    over_budget_out: &mut bool,
) -> Option<Decoded> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut pos = 2;
    let mut qt = [[0u16; 64]; 4];
    let mut dc_tables: [Option<Huffman>; 4] = Default::default();
    let mut ac_tables: [Option<Huffman>; 4] = Default::default();
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let (mut hmax, mut vmax) = (1usize, 1usize);
    let mut progressive = false;
    let mut restart_interval = 0usize;
    let mut saw_jfif = false;
    let mut adobe_transform: Option<u8> = None;
    let mut mcux = 0;
    let mut mcuy = 0;
    let mut frame_seen = false;

    loop {
        // Find the next marker.
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        let marker = *data.get(pos)?;
        pos += 1;
        match marker {
            0xD9 => break,
            0xD0..=0xD7 | 0x01 => continue,
            _ => {}
        }
        let len = be16(data, pos)?;
        let seg = data.get(pos + 2..pos + len)?;
        match marker {
            0xE0 => {
                if seg.starts_with(b"JFIF\0") {
                    saw_jfif = true;
                }
            }
            0xEE => {
                if seg.len() >= 12 && seg.starts_with(b"Adobe") {
                    adobe_transform = Some(seg[11]);
                }
            }
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let pq = seg[p] >> 4;
                    let tq = (seg[p] & 15) as usize;
                    if tq > 3 {
                        return None;
                    }
                    p += 1;
                    for k in 0..64 {
                        let v = if pq == 0 {
                            *seg.get(p + k)? as u16
                        } else {
                            be16(seg, p + 2 * k)? as u16
                        };
                        qt[tq][NATURAL[k]] = v;
                    }
                    p += if pq == 0 { 64 } else { 128 };
                }
            }
            0xC4 => {
                let mut p = 0;
                while p < seg.len() {
                    let tc = seg[p] >> 4;
                    let th = (seg[p] & 15) as usize;
                    if th > 3 {
                        return None;
                    }
                    let counts: [u8; 16] = seg.get(p + 1..p + 17)?.try_into().ok()?;
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let values = seg.get(p + 17..p + 17 + n)?.to_vec();
                    let table = Huffman::new(&counts, values)?;
                    if tc == 0 {
                        dc_tables[th] = Some(table);
                    } else {
                        ac_tables[th] = Some(table);
                    }
                    p += 17 + n;
                }
            }
            0xDD => restart_interval = be16(seg, 0)?,
            0xC0..=0xC2 => {
                if frame_seen || *seg.first()? != 8 {
                    return None;
                }
                frame_seen = true;
                progressive = marker == 0xC2;
                height = be16(seg, 1)?;
                width = be16(seg, 3)?;
                let nc = *seg.get(5)? as usize;
                if width == 0 || height == 0 || !(nc == 1 || nc == 3) {
                    return None;
                }
                for i in 0..nc {
                    let b = seg.get(6 + 3 * i..9 + 3 * i)?;
                    let (h, v) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) || b[2] > 3 {
                        return None;
                    }
                    comps.push(Component {
                        id: b[0],
                        h,
                        v,
                        tq: b[2] as usize,
                        bw: 0,
                        bh: 0,
                        real_bw: 0,
                        real_bh: 0,
                        dw: 0,
                        dh: 0,
                        coefs: Vec::new(),
                        dc_tbl: 0,
                        ac_tbl: 0,
                        coef_bits: [-1; 10],
                    });
                }
                hmax = comps.iter().map(|c| c.h).max()?;
                vmax = comps.iter().map(|c| c.v).max()?;
                mcux = width.div_ceil(8 * hmax);
                mcuy = height.div_ceil(8 * vmax);
                // First pass: work out every component's block geometry
                // without allocating anything yet, so the total cost can be
                // checked against MAX_DECODE_BUDGET_BYTES before any of it
                // is committed.
                for c in &mut comps {
                    c.dw = (width * c.h).div_ceil(hmax);
                    c.dh = (height * c.v).div_ceil(vmax);
                    c.real_bw = c.dw.div_ceil(8);
                    c.real_bh = c.dh.div_ceil(8);
                    c.bw = mcux * c.h;
                    c.bh = mcuy * c.v;
                }
                let mut coef_bytes: u64 = 0;
                let mut plane_bytes: u64 = 0;
                for c in &comps {
                    let blocks = (c.bw as u64).checked_mul(c.bh as u64)?;
                    let coefs = blocks.checked_mul(std::mem::size_of::<[i16; 64]>() as u64)?;
                    coef_bytes = coef_bytes.checked_add(coefs)?;
                    let plane = (c.bw as u64)
                        .checked_mul(8)?
                        .checked_mul(c.bh as u64)?
                        .checked_mul(8)?;
                    plane_bytes = plane_bytes.checked_add(plane)?;
                }
                let rgb_bytes = (width as u64).checked_mul(height as u64)?.checked_mul(3)?;
                // `full` (jpeg.rs, `upsample`'s per-component output below)
                // is one `width * height` buffer per component, live
                // alongside `planes` and then `rgb`: count it too, or the
                // real peak is roughly this budget plus 3*w*h uncounted.
                let upsample_bytes = (comps.len() as u64)
                    .checked_mul(width as u64)?
                    .checked_mul(height as u64)?;
                let total = coef_bytes
                    .checked_add(plane_bytes)?
                    .checked_add(rgb_bytes)?
                    .checked_add(upsample_bytes)?;
                if total > MAX_DECODE_BUDGET_BYTES {
                    // Distinct from every other bailout in this function:
                    // the header is well-formed but declares more than the
                    // documented budget. The caller must turn this into a
                    // hard error instead of trying another decoder on the
                    // same oversized dimensions.
                    *over_budget_out = true;
                    return None;
                }
                for c in &mut comps {
                    c.coefs = vec![[0i16; 64]; c.bw * c.bh];
                }
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => {
                if !frame_seen {
                    return None;
                }
                let ns = *seg.first()? as usize;
                let mut scomps = Vec::with_capacity(ns);
                for i in 0..ns {
                    let cid = *seg.get(1 + 2 * i)?;
                    let t = *seg.get(2 + 2 * i)?;
                    let ci = comps.iter().position(|c| c.id == cid)?;
                    comps[ci].dc_tbl = (t >> 4) as usize;
                    comps[ci].ac_tbl = (t & 15) as usize;
                    if comps[ci].dc_tbl > 3 || comps[ci].ac_tbl > 3 {
                        return None;
                    }
                    scomps.push(ci);
                }
                let ss = *seg.get(1 + 2 * ns)? as usize;
                let se = *seg.get(2 + 2 * ns)? as usize;
                let a = *seg.get(3 + 2 * ns)?;
                let (ah, al) = ((a >> 4) as u32, (a & 15) as u32);
                if ns == 0 || ns > 4 || se > 63 || ss > se {
                    return None;
                }
                let mut rd = Reader {
                    data,
                    pos: pos + len,
                    bits: 0,
                    nbits: 0,
                    hit_marker: false,
                    truncated: false,
                };
                let scan = Scan {
                    comps: &scomps,
                    ss,
                    se,
                    ah,
                    al,
                    progressive,
                    restart_interval,
                    mcux,
                    mcuy,
                };
                if progressive {
                    for &ci in &scomps {
                        let c = &mut comps[ci];
                        for k in ss..=se.min(9) {
                            c.coef_bits[k] = al as i32;
                        }
                    }
                }
                decode_scan(&scan, &mut comps, &dc_tables, &ac_tables, &mut rd)?;
                if rd.truncated {
                    // The entropy-coded data ran out before this scan
                    // finished. Rather than guess at libjpeg-turbo's fake-EOI
                    // recovery (which this decoder does not replicate byte
                    // for byte), report it so the caller can fail outright
                    // instead of letting the generic `image` crate fallback
                    // guess at its own recovery.
                    *truncated_out = true;
                    return None;
                }
                pos = rd.end_of_scan();
                continue;
            }
            _ => {}
        }
        pos += len;
    }
    if !frame_seen {
        return None;
    }
    if progressive && comps.iter().any(|c| c.coef_bits.iter().any(|&b| b != 0)) {
        // libjpeg-turbo would block-smooth this image; not replicated.
        return None;
    }

    // Dequantize + IDCT every block into per-component planes. Sized from
    // the same c.bw/c.bh already covered by the MAX_DECODE_BUDGET_BYTES
    // check above, so this can't outgrow that budget either.
    let planes: Vec<Vec<u8>> = comps
        .iter()
        .map(|c| {
            let pw = c.bw * 8;
            let mut plane = vec![0u8; pw * c.bh * 8];
            let q = &qt[c.tq];
            // One block row (8 sample rows) per work item.
            crate::extract::par_rows(&mut plane, pw * 8, |by, rows| {
                for bx in 0..c.bw {
                    let out = idct_islow(&c.coefs[by * c.bw + bx], q);
                    for r in 0..8 {
                        let o = r * pw + bx * 8;
                        rows[o..o + 8].copy_from_slice(&out[r * 8..r * 8 + 8]);
                    }
                }
            });
            plane
        })
        .collect();

    // Upsample each component to full size.
    let full: Vec<Vec<u8>> = comps
        .iter()
        .zip(&planes)
        .map(|(c, p)| upsample(c, p, hmax, vmax, width, height))
        .collect();

    if comps.len() == 1 {
        return Some(Decoded {
            width,
            height,
            channels: 1,
            data: full.into_iter().next()?,
        });
    }
    let is_rgb = if saw_jfif {
        false
    } else if let Some(t) = adobe_transform {
        t == 0
    } else {
        !(comps[0].id == 1 && comps[1].id == 2 && comps[2].id == 3)
            && comps[0].id == 82
            && comps[1].id == 71
            && comps[2].id == 66
    };
    // width*height*3 was already folded into the MAX_DECODE_BUDGET_BYTES
    // check above.
    let mut rgb = vec![0u8; width * height * 3];
    if is_rgb {
        for i in 0..width * height {
            rgb[3 * i] = full[0][i];
            rgb[3 * i + 1] = full[1][i];
            rgb[3 * i + 2] = full[2][i];
        }
    } else {
        ycc_to_rgb(&full, &mut rgb);
    }
    Some(Decoded {
        width,
        height,
        channels: 3,
        data: rgb,
    })
}

struct Scan<'a> {
    comps: &'a [usize],
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
    progressive: bool,
    restart_interval: usize,
    mcux: usize,
    mcuy: usize,
}

struct ScanState {
    dc_pred: [i32; 4],
    eobrun: u32,
}

fn decode_scan(
    scan: &Scan,
    comps: &mut [Component],
    dc_tables: &[Option<Huffman>; 4],
    ac_tables: &[Option<Huffman>; 4],
    rd: &mut Reader,
) -> Option<()> {
    let mut st = ScanState {
        dc_pred: [0; 4],
        eobrun: 0,
    };
    // Block coordinates visited, in scan order.
    let single = scan.comps.len() == 1;
    let total_units = if single {
        let c = &comps[scan.comps[0]];
        c.real_bw * c.real_bh
    } else {
        scan.mcux * scan.mcuy
    };
    let mut restarts_left = scan.restart_interval;
    for unit in 0..total_units {
        if scan.restart_interval > 0 {
            if restarts_left == 0 {
                rd.restart();
                st.dc_pred = [0; 4];
                st.eobrun = 0;
                restarts_left = scan.restart_interval;
            }
            restarts_left -= 1;
        }
        if single {
            let ci = scan.comps[0];
            let bx = unit % comps[ci].real_bw;
            let by = unit / comps[ci].real_bw;
            decode_block(
                scan, comps, ci, 0, bx, by, dc_tables, ac_tables, rd, &mut st,
            )?;
        } else {
            let mx = unit % scan.mcux;
            let my = unit / scan.mcux;
            for (si, &ci) in scan.comps.iter().enumerate() {
                let (h, v) = (comps[ci].h, comps[ci].v);
                for yy in 0..v {
                    for xx in 0..h {
                        decode_block(
                            scan,
                            comps,
                            ci,
                            si,
                            mx * h + xx,
                            my * v + yy,
                            dc_tables,
                            ac_tables,
                            rd,
                            &mut st,
                        )?;
                    }
                }
            }
        }
    }
    Some(())
}

#[allow(clippy::too_many_arguments)]
fn decode_block(
    scan: &Scan,
    comps: &mut [Component],
    ci: usize,
    si: usize,
    bx: usize,
    by: usize,
    dc_tables: &[Option<Huffman>; 4],
    ac_tables: &[Option<Huffman>; 4],
    rd: &mut Reader,
    st: &mut ScanState,
) -> Option<()> {
    let c = &mut comps[ci];
    let idx = by * c.bw + bx;
    let block = c.coefs.get_mut(idx)?;
    if !scan.progressive {
        let dc = dc_tables[c.dc_tbl].as_ref()?;
        let ac = ac_tables[c.ac_tbl].as_ref()?;
        let t = rd.decode(dc) as u32;
        let diff = rd.receive_extend(t);
        st.dc_pred[si] = st.dc_pred[si].wrapping_add(diff);
        block[0] = st.dc_pred[si] as i16;
        let mut k = 1;
        while k < 64 {
            let rs = rd.decode(ac);
            let r = (rs >> 4) as usize;
            let s = (rs & 15) as u32;
            if s != 0 {
                k += r;
                let v = rd.receive_extend(s);
                if k < 64 {
                    block[NATURAL[k]] = v as i16;
                }
            } else {
                if r != 15 {
                    break;
                }
                k += 15;
            }
            k += 1;
        }
        return Some(());
    }
    if scan.ss == 0 {
        // DC scan.
        if scan.ah == 0 {
            let dc = dc_tables[c.dc_tbl].as_ref()?;
            let t = rd.decode(dc) as u32;
            let diff = rd.receive_extend(t);
            st.dc_pred[si] = st.dc_pred[si].wrapping_add(diff);
            block[0] = ((st.dc_pred[si] as u32) << scan.al) as i32 as i16;
        } else if rd.get_bit() {
            block[0] |= (1i32 << scan.al) as i16;
        }
        return Some(());
    }
    let ac = ac_tables[c.ac_tbl].as_ref()?;
    if scan.ah == 0 {
        // AC first pass.
        if st.eobrun > 0 {
            st.eobrun -= 1;
            return Some(());
        }
        let mut k = scan.ss;
        while k <= scan.se {
            let rs = rd.decode(ac);
            let r = (rs >> 4) as u32;
            let s = (rs & 15) as u32;
            if s != 0 {
                k += r as usize;
                let v = rd.receive_extend(s);
                if k < 64 {
                    block[NATURAL[k]] = ((v as u32) << scan.al) as i32 as i16;
                }
            } else if r == 15 {
                k += 15;
            } else {
                st.eobrun = 1 << r;
                if r != 0 {
                    st.eobrun += rd.get_bits(r) as u32;
                }
                st.eobrun -= 1;
                break;
            }
            k += 1;
        }
        return Some(());
    }
    // AC refinement.
    let p1 = 1i32 << scan.al;
    let m1 = (-1i32) << scan.al;
    let mut k = scan.ss;
    let refine = |coef: &mut i16, rd: &mut Reader| {
        if *coef != 0 && rd.get_bit() && (*coef as i32 & p1) == 0 {
            if *coef >= 0 {
                *coef = (*coef as i32 + p1) as i16;
            } else {
                *coef = (*coef as i32 + m1) as i16;
            }
        }
    };
    if st.eobrun == 0 {
        while k <= scan.se {
            let rs = rd.decode(ac);
            let mut r = (rs >> 4) as i32;
            let s_bits = (rs & 15) as u32;
            let mut s = 0i32;
            if s_bits != 0 {
                s = if rd.get_bit() { p1 } else { m1 };
            } else if r != 15 {
                st.eobrun = 1 << r;
                if r != 0 {
                    st.eobrun += rd.get_bits(r as u32) as u32;
                }
                break;
            }
            loop {
                let coef = &mut block[NATURAL[k.min(63)]];
                if *coef != 0 {
                    refine(coef, rd);
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
                if k > scan.se {
                    break;
                }
            }
            if s != 0 && k < 64 {
                block[NATURAL[k]] = s as i16;
            }
            k += 1;
        }
    }
    if st.eobrun > 0 {
        while k <= scan.se {
            refine(&mut block[NATURAL[k]], rd);
            k += 1;
        }
        st.eobrun -= 1;
    }
    Some(())
}

/// `jpeg_idct_islow` + the post-IDCT range limit.
fn idct_islow(coef: &[i16; 64], quant: &[u16; 64]) -> [u8; 64] {
    const CONST_BITS: i64 = 13;
    const PASS1_BITS: i64 = 2;
    const F0_541: i64 = 4433;
    const F0_765: i64 = 6270;
    const F1_847: i64 = 15137;
    let descale = |x: i64, n: i64| (x + (1i64 << (n - 1))) >> n;
    let deq = |i: usize| coef[i] as i64 * quant[i] as i64;
    let mut ws = [0i32; 64];
    for col in 0..8 {
        if (1..8).all(|r| coef[r * 8 + col] == 0) {
            let dc = (deq(col) << PASS1_BITS) as i32;
            for r in 0..8 {
                ws[r * 8 + col] = dc;
            }
            continue;
        }
        let z2 = deq(2 * 8 + col);
        let z3 = deq(6 * 8 + col);
        let z1 = (z2 + z3) * F0_541;
        let tmp2 = z1 + z3 * -F1_847;
        let tmp3 = z1 + z2 * F0_765;
        let z2 = deq(col);
        let z3 = deq(4 * 8 + col);
        let tmp0 = (z2 + z3) << CONST_BITS;
        let tmp1 = (z2 - z3) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let (t0, t1, t2, t3) = (
            deq(7 * 8 + col),
            deq(5 * 8 + col),
            deq(3 * 8 + col),
            deq(8 + col),
        );
        let (o0, o1, o2, o3) = odd_part(t0, t1, t2, t3);
        let n = CONST_BITS - PASS1_BITS;
        ws[col] = descale(tmp10 + o3, n) as i32;
        ws[7 * 8 + col] = descale(tmp10 - o3, n) as i32;
        ws[8 + col] = descale(tmp11 + o2, n) as i32;
        ws[6 * 8 + col] = descale(tmp11 - o2, n) as i32;
        ws[2 * 8 + col] = descale(tmp12 + o1, n) as i32;
        ws[5 * 8 + col] = descale(tmp12 - o1, n) as i32;
        ws[3 * 8 + col] = descale(tmp13 + o0, n) as i32;
        ws[4 * 8 + col] = descale(tmp13 - o0, n) as i32;
    }
    let mut out = [0u8; 64];
    let n = CONST_BITS + PASS1_BITS + 3;
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let o = &mut out[row * 8..row * 8 + 8];
        if w[1..].iter().all(|&v| v == 0) {
            let dc = range_limit(descale(w[0] as i64, PASS1_BITS + 3));
            o.fill(dc);
            continue;
        }
        let z2 = w[2] as i64;
        let z3 = w[6] as i64;
        let z1 = (z2 + z3) * F0_541;
        let tmp2 = z1 + z3 * -F1_847;
        let tmp3 = z1 + z2 * F0_765;
        let tmp0 = (w[0] as i64 + w[4] as i64) << CONST_BITS;
        let tmp1 = (w[0] as i64 - w[4] as i64) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let (o0, o1, o2, o3) = odd_part(w[7] as i64, w[5] as i64, w[3] as i64, w[1] as i64);
        o[0] = range_limit(descale(tmp10 + o3, n));
        o[7] = range_limit(descale(tmp10 - o3, n));
        o[1] = range_limit(descale(tmp11 + o2, n));
        o[6] = range_limit(descale(tmp11 - o2, n));
        o[2] = range_limit(descale(tmp12 + o1, n));
        o[5] = range_limit(descale(tmp12 - o1, n));
        o[3] = range_limit(descale(tmp13 + o0, n));
        o[4] = range_limit(descale(tmp13 - o0, n));
    }
    out
}

/// The odd part of the islow 1-D kernel (inputs y7, y5, y3, y1), with
/// jidctint.c's `FIX_*` constants (CONST_BITS 13).
#[inline]
fn odd_part(tmp0: i64, tmp1: i64, tmp2: i64, tmp3: i64) -> (i64, i64, i64, i64) {
    let z1 = tmp0 + tmp3;
    let z2 = tmp1 + tmp2;
    let z3 = tmp0 + tmp2;
    let z4 = tmp1 + tmp3;
    let z5 = (z3 + z4) * 9633;
    let mut t0 = tmp0 * 2446;
    let mut t1 = tmp1 * 16819;
    let mut t2 = tmp2 * 25172;
    let mut t3 = tmp3 * 12299;
    let z1 = z1 * -7373;
    let z2 = z2 * -20995;
    let z3 = z3 * -16069 + z5;
    let z4 = z4 * -3196 + z5;
    t0 += z1 + z3;
    t1 += z2 + z4;
    t2 += z2 + z3;
    t3 += z1 + z4;
    (t0, t1, t2, t3)
}

/// `IDCT_range_limit(cinfo)[x & RANGE_MASK]` for 8-bit samples.
#[inline]
fn range_limit(x: i64) -> u8 {
    let t = (x & 1023) as usize;
    match t {
        0..=127 => (t + 128) as u8,
        128..=511 => 255,
        512..=895 => 0,
        _ => (t - 896) as u8,
    }
}

/// Upsample one component to `width x height` (`jdsample.c` +
/// `jdmainct.c` context handling).
fn upsample(
    c: &Component,
    plane: &[u8],
    hmax: usize,
    vmax: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let pw = c.bw * 8;
    let mut out = vec![0u8; width * height];
    let (dw, dh) = (c.dw, c.dh);
    // Sample row r of the component, with the first/last real row
    // duplicated beyond the edges.
    let row = |r: isize| -> &[u8] {
        let r = r.clamp(0, dh as isize - 1) as usize;
        &plane[r * pw..r * pw + pw]
    };
    let fancy_h2 = dw > 2;
    if c.h == hmax && c.v == vmax {
        for y in 0..height {
            out[y * width..(y + 1) * width].copy_from_slice(&row(y as isize)[..width]);
        }
    } else if c.h * 2 == hmax && c.v == vmax {
        let mut line = vec![0u8; dw * 2];
        for y in 0..height {
            let inp = row(y as isize);
            if fancy_h2 {
                h2v1_fancy(&inp[..dw], &mut line);
            } else {
                for (i, &v) in inp[..dw].iter().enumerate() {
                    line[2 * i] = v;
                    line[2 * i + 1] = v;
                }
            }
            out[y * width..(y + 1) * width].copy_from_slice(&line[..width]);
        }
    } else if c.h == hmax && c.v * 2 == vmax {
        for y in 0..height {
            let inrow = (y / 2) as isize;
            let (near, far, bias) = if y % 2 == 0 {
                (row(inrow), row(inrow - 1), 1)
            } else {
                (row(inrow), row(inrow + 1), 2)
            };
            for x in 0..width {
                out[y * width + x] = ((near[x] as i32 * 3 + far[x] as i32 + bias) >> 2) as u8;
            }
        }
    } else if c.h * 2 == hmax && c.v * 2 == vmax {
        let mut line = vec![0u8; dw * 2];
        for y in 0..height {
            let inrow = (y / 2) as isize;
            if fancy_h2 {
                let near = &row(inrow)[..dw];
                let far = if y % 2 == 0 {
                    &row(inrow - 1)[..dw]
                } else {
                    &row(inrow + 1)[..dw]
                };
                h2v2_fancy(near, far, &mut line);
            } else {
                for (i, &v) in row(inrow)[..dw].iter().enumerate() {
                    line[2 * i] = v;
                    line[2 * i + 1] = v;
                }
            }
            out[y * width..(y + 1) * width].copy_from_slice(&line[..width]);
        }
    } else {
        // int_upsample: plain replication by integral factors.
        let (fx, fy) = (hmax / c.h, vmax / c.v);
        for y in 0..height {
            let inp = row((y / fy) as isize);
            for x in 0..width {
                out[y * width + x] = inp[x / fx];
            }
        }
    }
    out
}

fn h2v1_fancy(inp: &[u8], out: &mut [u8]) {
    let n = inp.len();
    let v = inp[0] as i32;
    out[0] = v as u8;
    out[1] = ((v * 3 + inp[1] as i32 + 2) >> 2) as u8;
    for i in 1..n - 1 {
        let v = inp[i] as i32 * 3;
        out[2 * i] = ((v + inp[i - 1] as i32 + 1) >> 2) as u8;
        out[2 * i + 1] = ((v + inp[i + 1] as i32 + 2) >> 2) as u8;
    }
    let v = inp[n - 1] as i32;
    out[2 * n - 2] = ((v * 3 + inp[n - 2] as i32 + 1) >> 2) as u8;
    out[2 * n - 1] = v as u8;
}

fn h2v2_fancy(near: &[u8], far: &[u8], out: &mut [u8]) {
    let n = near.len();
    let col = |i: usize| near[i] as i32 * 3 + far[i] as i32;
    let mut this = col(0);
    let mut next = col(1);
    out[0] = ((this * 4 + 8) >> 4) as u8;
    out[1] = ((this * 3 + next + 7) >> 4) as u8;
    let mut last = this;
    this = next;
    for i in 1..n - 1 {
        next = col(i + 1);
        out[2 * i] = ((this * 3 + last + 8) >> 4) as u8;
        out[2 * i + 1] = ((this * 3 + next + 7) >> 4) as u8;
        last = this;
        this = next;
    }
    out[2 * n - 2] = ((this * 3 + last + 8) >> 4) as u8;
    out[2 * n - 1] = ((this * 4 + 7) >> 4) as u8;
}

/// `ycc_rgb_convert` with `build_ycc_rgb_table`'s fixed-point tables.
fn ycc_to_rgb(planes: &[Vec<u8>], rgb: &mut [u8]) {
    const SCALEBITS: i64 = 16;
    const ONE_HALF: i64 = 1 << (SCALEBITS - 1);
    let fix = |x: f64| (x * (1i64 << SCALEBITS) as f64 + 0.5) as i64;
    let mut cr_r = [0i32; 256];
    let mut cb_b = [0i32; 256];
    let mut cr_g = [0i64; 256];
    let mut cb_g = [0i64; 256];
    for i in 0..256 {
        let x = i as i64 - 128;
        cr_r[i] = ((fix(1.40200) * x + ONE_HALF) >> SCALEBITS) as i32;
        cb_b[i] = ((fix(1.77200) * x + ONE_HALF) >> SCALEBITS) as i32;
        cr_g[i] = -fix(0.71414) * x;
        cb_g[i] = -fix(0.34414) * x + ONE_HALF;
    }
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    for i in 0..planes[0].len() {
        let y = planes[0][i] as i32;
        let cb = planes[1][i] as usize;
        let cr = planes[2][i] as usize;
        rgb[3 * i] = clamp(y + cr_r[cr]);
        rgb[3 * i + 1] = clamp(y + ((cb_g[cb] + cr_g[cr]) >> SCALEBITS) as i32);
        rgb[3 * i + 2] = clamp(y + cb_b[cb]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/jpeg")
                .join(name),
        )
        .unwrap()
    }

    /// Pixels of a binary PPM (as written by `magick ... ppm:`).
    fn ppm_pixels(ppm: &[u8]) -> &[u8] {
        let mut newlines = 0;
        for (i, &b) in ppm.iter().enumerate() {
            if b == b'\n' {
                newlines += 1;
                if newlines == 3 {
                    return &ppm[i + 1..];
                }
            }
        }
        panic!("not a PPM");
    }

    /// The references are ImageMagick's own decode (libjpeg-turbo), so
    /// these check bit-exactness, not just plausibility: odd sizes (edge
    /// blocks, last-column/row upsampling), 4:2:0 progressive, and 4:2:2
    /// with a restart marker after every MCU.
    #[test]
    fn decodes_bit_identically_to_libjpeg_turbo() {
        for name in ["odd-420-progressive", "odd-422-restart"] {
            let d = decode(&fixture(&format!("{name}.jpg")), &mut false, &mut false)
                .expect("supported JPEG");
            assert_eq!((d.width, d.height, d.channels), (45, 37, 3));
            let reference = fixture(&format!("{name}.ppm"));
            assert!(
                d.data == ppm_pixels(&reference),
                "{name}: decode differs from libjpeg-turbo"
            );
        }
    }

    #[test]
    fn idct_of_dc_only_block_is_flat() {
        let mut coef = [0i16; 64];
        coef[0] = 16;
        let out = idct_islow(&coef, &[1u16; 64]);
        // DC 16 * quant 1 -> 16 / 8 = 2 above the 128 centre.
        assert!(out.iter().all(|&v| v == 130));
    }

    #[test]
    fn range_limit_wraps_like_the_post_idct_table() {
        assert_eq!(range_limit(0), 128);
        assert_eq!(range_limit(127), 255);
        assert_eq!(range_limit(400), 255);
        assert_eq!(range_limit(-1), 127);
        assert_eq!(range_limit(-128), 0);
        assert_eq!(range_limit(-300), 0);
    }

    #[test]
    fn unsupported_or_malformed_input_is_none() {
        assert!(decode(b"not a jpeg", &mut false, &mut false).is_none());
        assert!(decode(&[0xFF, 0xD8, 0xFF], &mut false, &mut false).is_none());
        // Arithmetic-coded frame marker (SOF9) is left to the fallback.
        assert!(decode(
            &[0xFF, 0xD8, 0xFF, 0xC9, 0x00, 0x02],
            &mut false,
            &mut false
        )
        .is_none());
    }

    /// A ~20-byte file whose SOF0 declares 65500x65500 (one grayscale
    /// component): the exact shape of the crafted input that drove RSS to
    /// 11.4 GB before MAX_DECODE_BUDGET_BYTES existed. The declared
    /// coefficient/plane/output buffers must be rejected before any of
    /// them is allocated, not after, and the rejection must be reported as
    /// `over_budget_out`, not just a generic `None`, so the caller does not
    /// fall through to a second, unbounded decoder on the same dimensions.
    #[test]
    fn huge_sof_dimensions_are_rejected_before_allocating() {
        let data = [
            0xFF, 0xD8, // SOI
            0xFF, 0xC0, 0x00, 0x0B, // SOF0, segment length 11
            0x08, // precision
            0xFF, 0xDC, // height 65500
            0xFF, 0xDC, // width 65500
            0x01, // Nc = 1
            0x01, 0x11, 0x00, // component: id 1, h=v=1, quant table 0
            0xFF, 0xD9, // EOI
        ];
        let mut over_budget = false;
        assert!(decode(&data, &mut false, &mut over_budget).is_none());
        assert!(
            over_budget,
            "must be reported as over budget, not a plain None"
        );
    }

    #[test]
    fn truncated_scan_data_is_rejected_not_guessed_at() {
        let full = fixture("odd-420-progressive.jpg");
        assert!(
            decode(&full, &mut false, &mut false).is_some(),
            "fixture must decode when whole"
        );
        // Cut well past the headers and into the middle of the
        // entropy-coded scan data: libjpeg-turbo would synthesize a fake
        // EOI and keep going with a warning, which this decoder does not
        // replicate. Returning `None` here is safer than silently
        // returning plausible-looking but wrong colours. Whether this
        // particular cut is detected via `truncated_out` (mid-scan) or an
        // earlier malformed-header bailout depends on exactly where a
        // multi-scan progressive file gets cut; either way `decode` must
        // refuse it. `crate::extract::tests::undecodable_input_is_an_error`
        // covers the `truncated_out` flag itself on a single-scan (baseline)
        // fixture, where a mid-file cut deterministically lands inside the
        // one scan's entropy data.
        let truncated = &full[..full.len() * 7 / 10];
        assert!(decode(truncated, &mut false, &mut false).is_none());
    }
}
