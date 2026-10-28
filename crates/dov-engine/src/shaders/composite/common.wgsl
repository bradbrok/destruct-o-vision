// Composite-domain helpers. The composite signal is a frame of
// `lines` rows × `spl` samples covering the whole scan, blanking included,
// sampled at the raster's clock. Each field writes its own rows.

struct Comp {
    // Samples per line, lines per frame.
    spl: i32,
    lines: i32,
    // Rows written by this field.
    row0: i32,
    rows: i32,
    active_start: i32,
    active_samples: i32,
    // Lines from the start of a field to its first picture line.
    first_active: i32,
    // First row of the second field (`lines` when progressive).
    field2_row: i32,
    // Subcarrier cycles per line (fractional part) and per sample.
    line_fract: f32,
    sample_inc: f32,
    pal: bool,
    color: bool,
    // Samples a second that the standard's sync and burst timings (µs) are
    // counted at, and that bandwidths (MHz) and times across the picture
    // are: the raster's own clock for NTSC and PAL; for other rasters,
    // NTSC's timing scaled to fit (see `RasterStd::composite`).
    sync_fs: f32,
    fs: f32,
    // NTSC scaled onto another raster, rather than a standard's own.
    scaled: bool,
}

// Filled by the engine in u.extra[4..8].
fn comp() -> Comp {
    var c: Comp;
    let t = u.extra[7];
    let a = u.extra[6];
    let sc = u.extra[5];
    c.spl = i32(t.x);
    c.lines = i32(t.y);
    c.row0 = i32(t.z);
    c.rows = i32(t.w);
    c.active_start = i32(a.x);
    c.active_samples = i32(a.y);
    c.first_active = i32(a.z);
    c.field2_row = i32(a.w);
    c.line_fract = sc.x;
    c.sample_inc = sc.y;
    c.pal = sc.z > 0.5;
    c.color = sc.y > 0.0;
    c.sync_fs = u.extra[4].y;
    c.fs = u.extra[4].z;
    c.scaled = u.extra[4].w > 0.5;
    return c;
}

// Levels: blanking 0, peak white 1 (100 IRE), sync tip -0.4 (-40 IRE).
const SYNC_LEVEL: f32 = -0.4;
const BURST_LEVEL: f32 = 0.2;
const HSYNC_US: f32 = 4.7;
const EQ_US: f32 = 2.3;
const SERRATION_US: f32 = 4.7;
const BURST_START_US: f32 = 5.3;
const BURST_US: f32 = 2.5;
// Half-lines of equalizing, broad (vertical sync) and equalizing pulses.
const V_INTERVAL: i32 = 18;

// Samples in `t` µs of the standard's sync and blanking.
fn us(c: Comp, t: f32) -> f32 {
    return t * 1e-6 * c.sync_fs;
}

// Samples in `t` µs across the picture.
fn picture_us(c: Comp, t: f32) -> f32 {
    return t * 1e-6 * c.fs;
}

// Samples that `n` samples of NTSC's picture (at 13.5 MHz) stretch across,
// for shifts and lengths counted in samples.
fn samples(c: Comp, n: f32) -> f32 {
    return n * c.fs / 13.5e6;
}

// NTSC sits black at 7.5 IRE above blanking; PAL and HD don't.
fn setup(c: Comp) -> f32 {
    return select(0.0, 0.075, c.color && !c.pal);
}

// One-pole low-pass coefficient for a cutoff in MHz.
fn lp_coef(c: Comp, mhz: f32) -> f32 {
    return 1.0 - exp(-TAU * max(mhz, 0.001) * 1e6 / c.fs);
}

// Subcarrier phase in cycles at (row, sample) for an oscillator with the
// given phase at row 0, cycles per line (fraction) and cycles per sample.
// Continuous across rows because `line_fract` matches `inc` × samples per line.
fn carrier(base: f32, line_fract: f32, inc: f32, row: i32, s: f32) -> f32 {
    return fract(base + fract(f32(row) * line_fract) + s * inc);
}

// The receiver's reference oscillator: nominal frequency, phase 0 at row 0.
fn reference(c: Comp, row: i32, s: f32) -> f32 {
    return carrier(0.0, c.line_fract, c.sample_inc, row, s);
}

// (line within its field, field 0 or 1) for a composite row.
fn field_line(c: Comp, row: i32) -> vec2<i32> {
    if (row >= c.field2_row) {
        return vec2<i32>(row - c.field2_row, 1);
    }
    return vec2<i32>(row, 0);
}

fn field_rows(c: Comp, field: i32) -> i32 {
    return select(c.lines - c.field2_row, c.field2_row, field == 0);
}

// What the standard puts at (row, sample). x: the sync/blanking level;
// y: 0 sync or blanking, 1 inside the burst gate, 2 active picture.
fn standard_at(c: Comp, row: i32, s: i32) -> vec2<f32> {
    let half = c.spl / 2;
    let hl = row * 2 + select(0, 1, s >= half);
    var k = hl;
    // Interlaced: the second field's vertical interval starts mid-line.
    if (c.field2_row < c.lines && hl >= c.lines) {
        k = hl - c.lines;
    }
    let hs = f32(s % half);
    if (k < V_INTERVAL) {
        if (k < 6 || k >= 12) {
            return vec2<f32>(select(0.0, SYNC_LEVEL, hs < us(c, EQ_US)), 0.0);
        }
        return vec2<f32>(select(0.0, SYNC_LEVEL, hs < f32(half) - us(c, SERRATION_US)), 0.0);
    }
    if (f32(s) < us(c, HSYNC_US)) {
        return vec2<f32>(SYNC_LEVEL, 0.0);
    }
    let line = field_line(c, row).x - c.first_active;
    if (line >= 0 && line < i32(u.info.lines) && s >= c.active_start && s < c.active_start + c.active_samples) {
        return vec2<f32>(0.0, 2.0);
    }
    let b = f32(s) - us(c, BURST_START_US);
    if (b >= 0.0 && b < us(c, BURST_US)) {
        return vec2<f32>(0.0, 1.0);
    }
    return vec2<f32>(0.0, 0.0);
}

fn rnd(a: u32, b: u32, c: u32) -> f32 {
    return hash3(vec3<u32>(a, b, c));
}
