// Tube pass 4, the screen, at the target's resolution: the lines' light,
// spread down the screen by the beam spot, lands on the phosphors through
// the mask and leaves the glass with its halation and the room's
// reflection. All in linear light until the very end.

struct View {
    // Fraction of the target the tube face covers, per axis.
    scale: vec2<f32>,
    // Target size in pixels.
    size: vec2<f32>,
}

@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var beam: texture_2d<f32>;
@group(0) @binding(3) var sigma: texture_2d<f32>;
@group(0) @binding(4) var glow: texture_2d<f32>;
@group(0) @binding(5) var average: texture_2d<f32>;
@group(0) @binding(6) var yoke: texture_2d<f32>;
@group(0) @binding(7) var samp: sampler;

const TAU: f32 = 6.2831853;
// Lines one pixel may gather before it just takes their average.
const MAX_ROWS: i32 = 48;
// Phosphor stripes and dots: half their width (of a stripe's third of a
// triad) and radius (of the spacing between dots).
const STRIPE: f32 = 0.4;
const DOT: f32 = 0.42;
// Height of a slot mask's slots, in triads, and of the bridges between.
const SLOT: f32 = 1.0;
const BRIDGE: f32 = 0.1;
// The degaussing coil's field dies away over this long (see dov_modules::crt).
const DEGAUSS_SECONDS: f32 = 2.0;
// Where the magnet sits: just off the right side of the set.
const MAGNET_AT = vec2<f32>(1.35, 0.3);

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the screen: (-1,-1), (3,-1), (-1,3).
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// The normal distribution's CDF, per channel (erf after Winitzki).
fn phi(x: vec3<f32>) -> vec3<f32> {
    let z = x * 0.70710678;
    let z2 = z * z;
    let a = 0.147;
    let e = sqrt(1.0 - exp(-z2 * (1.2732395 + a * z2) / (1.0 + a * z2)));
    return 0.5 + 0.5 * sign(z) * e;
}

// ------------------------------------------------------------------ mask

// Vertical phosphor stripes, R G B across each triad. `x` is in triads;
// `soft` is an edge's half-width, in stripes.
fn stripes(x: f32, soft: f32) -> vec3<f32> {
    let centers = vec3<f32>(1.0, 3.0, 5.0) / 6.0;
    let d = abs(fract(x - centers + 0.5) - 0.5) * 3.0;
    return 1.0 - smoothstep(vec3<f32>(STRIPE - soft), vec3<f32>(STRIPE + soft), d);
}

// A slot mask: stripes cut into slots, staggered from one triad to the next.
fn slots(m: vec2<f32>, soft: f32) -> vec3<f32> {
    let stagger = 0.5 * (floor(m.x) - 2.0 * floor(floor(m.x) * 0.5));
    let y = fract(m.y / SLOT + stagger);
    let open = 1.0 - smoothstep(0.5 - BRIDGE - soft, 0.5 - BRIDGE + soft, abs(y - 0.5));
    return stripes(m.x, soft) * open;
}

// A shadow mask: round dots, R G B along each row with the next row
// shifted half a dot, so every three neighbors make a triad.
fn dots(m: vec2<f32>, soft: f32) -> vec3<f32> {
    let d = 1.0 / 3.0;
    let h = d * 0.8660254;
    let b = m.y / h;
    let a = m.x / d - 0.5 * b;
    let a0 = floor(a);
    let b0 = floor(b);
    var c = vec3<f32>(0.0);
    for (var i = 0; i < 4; i = i + 1) {
        let ai = a0 + f32(i & 1);
        let bi = b0 + f32(i >> 1u);
        let center = vec2<f32>((ai + 0.5 * bi) * d, bi * h);
        let r = length(m - center) / d;
        let cover = 1.0 - smoothstep(DOT - soft, DOT + soft, r);
        let k = ai - bi - 3.0 * floor((ai - bi) / 3.0);
        c = c + cover * vec3<f32>(f32(k < 0.5), f32(k > 0.5 && k < 1.5), f32(k > 1.5));
    }
    return c;
}

// Peak over average of each mask at full strength: one over the share of
// the face its phosphors of one color cover (see MASK_GAIN in tube.rs).
fn mask_gain(kind: u32) -> f32 {
    return select(4.6875, 3.75, kind == 0u);
}

// How much light the mask lets through at face point `q`, per color, as a
// gain that averages 1. Averaged over the pixel, so it neither aliases
// nor flickers; a mask too fine for the target fades to its average.
fn mask(q: vec2<f32>, px: vec2<f32>) -> vec3<f32> {
    let kind = u32(t.mask.x + 0.5);
    let triads = t.mask.z;
    let fade = 1.0 - smoothstep(0.3, 0.5, triads * px.x * 0.5);
    let amount = t.mask.y * fade;
    if (kind > 2u || amount <= 0.0) {
        return vec3<f32>(1.0);
    }
    // In triads, the same pitch both ways.
    let m = (q * vec2<f32>(0.5, -0.5) + 0.5) * vec2<f32>(triads, triads / t.geom.w);
    let span = triads * px * vec2<f32>(0.5, 0.5 / t.geom.w);
    let soft = min(span.x * 0.5 * 3.0, 0.12);
    // Samples across the pixel: fewer where the mask is coarse next to it,
    // and a grille is the same all the way down.
    let n = select(3, 2, span.x < 0.2);
    let down = select(n, 1, kind == 0u);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < down; j = j + 1) {
        for (var i = 0; i < n; i = i + 1) {
            let s = m + (vec2<f32>(f32(i), f32(j)) + 0.5) / vec2<f32>(f32(n), f32(down)) * span - 0.5 * span;
            switch kind {
                case 0u: { sum = sum + stripes(s.x, soft); }
                case 1u: { sum = sum + dots(s, soft); }
                default: { sum = sum + slots(s, soft); }
            }
        }
    }
    return mix(vec3<f32>(1.0), sum / f32(n * down) * mask_gain(kind), amount);
}

// -------------------------------------------------------------- magnetism

fn degaussing() -> f32 {
    let s = t.events.z;
    return select(0.0, exp(-s / 0.55), t.events.y > 0.5 && s < DEGAUSS_SECONDS);
}

// How strongly a magnet by the set reaches face point `q`, 0..1.
fn magnet_reach(q: vec2<f32>) -> f32 {
    let d = (q - MAGNET_AT) * vec2<f32>(t.geom.w, 1.0);
    return pow(0.3 / (0.3 + dot(d, d)), 1.5);
}

// Smooth blotches, like a magnetized mask's, worst toward the corners.
fn blotches(q: vec2<f32>) -> f32 {
    let a = sin(q.x * 2.1 + 0.7) * sin(q.y * 2.7 - 0.4) + 0.6 * sin(q.x * 3.9 - q.y * 2.3 + 1.9)
        + 0.35 * sin(q.x * 6.1 + q.y * 5.3);
    return a * (0.45 + 0.275 * dot(q, q));
}

// Whether anything magnetic is going on.
fn magnetic() -> bool {
    return t.magnet.x != 0.0 || t.events.w != 0.0 || degaussing() > 0.0;
}

// How far the beams land from their own phosphors, in stripes: pulled
// by the magnet, and by what it left magnetized in the mask, mostly where
// it was held.
fn purity(q: vec2<f32>) -> f32 {
    let reach = magnet_reach(q);
    let left = t.events.w * (reach * (2.2 + 0.8 * blotches(q * 1.7)) + 0.3 * blotches(q));
    var e = 4.0 * t.magnet.x * reach + left;
    let s = t.events.z;
    e = e + degaussing() * 2.4 * sin(q.x * 2.3 + q.y * 1.7 + s * 7.0) * cos(q.y * 2.9 - s * 5.0);
    return e;
}

// Where the beams' light lands with the field bending them `e` stripes: a
// beam pushed a stripe over lights the next color round (red on green, and
// so on). The three guns sit apart, so the field bends each differently,
// which is what tints even a white picture.
fn land(c: vec3<f32>, e: f32) -> vec3<f32> {
    let shift = e * vec3<f32>(1.0, 0.4, -0.8);
    var out = vec3<f32>(0.0);
    for (var i = 0; i < 3; i = i + 1) {
        let at = f32(i) + shift[i];
        let k = at - 3.0 * floor(at / 3.0);
        let f = fract(k);
        let a = u32(floor(k)) % 3u;
        out[a] = out[a] + c[i] * (1.0 - f);
        out[(a + 1u) % 3u] = out[(a + 1u) % 3u] + c[i] * f;
    }
    return out;
}

// Where magnetic fields push the raster, in face units.
fn deflected(q: vec2<f32>) -> vec2<f32> {
    // The electrons swerve across a magnet's field: the picture twists
    // round it.
    let f = t.magnet.x * magnet_reach(q);
    let d = normalize((q - MAGNET_AT) * vec2<f32>(t.geom.w, 1.0));
    var s = f * (0.16 * vec2<f32>(-d.y, d.x) + 0.04 * d) * vec2<f32>(1.0 / t.geom.w, 1.0);
    // The degaussing coil's mains-frequency field shakes it about.
    let g = degaussing();
    let time = t.events.z;
    s = s + g * 0.012 * vec2<f32>(sin(TAU * 50.0 * time + q.y * 6.0), sin(TAU * 50.0 * time + 1.3 + q.x * 5.0));
    s = s + g * 0.03 * vec2<f32>(sin(TAU * 2.7 * time + q.y * 2.5), sin(TAU * 2.1 * time + q.x * 2.0));
    return s;
}

// ------------------------------------------------------------------- set

// The raster's size across and down and the beam's brightness, as the set
// warms up or is switched off.
fn power() -> vec3<f32> {
    let s = t.events.x;
    if (t.events.y > 0.5) {
        // The cathodes warm up and the high voltage comes up.
        let warm = smoothstep(0.0, 1.2, s);
        let size = 1.0 - 0.06 * (1.0 - smoothstep(0.0, 0.9, s));
        return vec3<f32>(size, size, warm);
    }
    // Switched off: down collapses first, then across, leaving a bright dot
    // that fades.
    let down = max(1.0 - s / 0.09, 0.006);
    let across = max(1.0 - max(s - 0.07, 0.0) / 0.18, 0.006);
    let fade = exp(-max(s - 0.25, 0.0) / 0.5);
    return vec3<f32>(across, down, min(1.0 / (across * down), 30.0) * fade);
}

// The picture's average light, for the high voltage's sag.
fn average_light() -> f32 {
    var sum = 0.0;
    for (var j = 0; j < 3; j = j + 1) {
        for (var i = 0; i < 3; i = i + 1) {
            let uv = vec2<f32>(0.2 + 0.3 * f32(i), 0.2 + 0.3 * f32(j));
            sum = sum + dot(textureSampleLevel(average, samp, uv, 0.0).rgb, LUMA);
        }
    }
    return sum / (9.0 * t.scan.y);
}

// How much of a pixel is glass, not bezel: a rounded rectangle.
fn glass(q: vec2<f32>, px: f32) -> f32 {
    let r = 0.05 + 0.07 * t.geom.x;
    let extent = vec2<f32>(t.geom.w, 1.0);
    let d = abs(q * extent) - (extent - r);
    let sd = length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
    return clamp(0.5 - sd / px, 0.0, 1.0);
}

// The room, reflected in the glass: the faceplate's gray glass and unlit
// phosphor, and a window somewhere behind you.
fn room(q: vec2<f32>) -> vec3<f32> {
    let level = t.glass.y;
    let w = (q - vec2<f32>(-0.45, 0.5)) * vec2<f32>(1.0, 1.6) * (1.0 + t.geom.x);
    let window = exp(-dot(w, w) * 3.0);
    return level * (vec3<f32>(0.006, 0.0065, 0.007) + vec3<f32>(0.025, 0.026, 0.028) * window);
}

fn hash(p: vec2<u32>) -> f32 {
    var h = (p.x * 0x8da6b343u) ^ (p.y * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    return f32(h ^ (h >> 15u)) / 4294967296.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The tube face, -1..1 across the picture area, y up; and one target
    // pixel in face units.
    let q = in.ndc / view.scale;
    let px = 2.0 / (view.size * view.scale);
    let color = t.decay.w < 0.5;

    // Seen through the curved glass, the raster bulges. Aperture grille
    // tubes are cylinders, curved across but not down.
    let k = 0.12 * t.geom.x;
    var bulge = vec2<f32>(dot(q, q));
    if (color && t.mask.x < 0.5) {
        bulge = vec2<f32>(1.3, 0.35) * q.x * q.x;
    }
    var p = q * (1.0 + k * bulge);
    // The raster overscans the glass, and grows as a bright picture makes
    // the high voltage sag.
    p = p / (t.geom.y * (1.0 + t.glass.z * average_light()));
    let on = power();
    p = p / on.xy;
    if (magnetic()) {
        p = p - deflected(q);
    }
    var uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    // The yoke: H pos and V pos, CV and all.
    let yoked = textureSampleLevel(yoke, samp, uv, 0.0).xy;
    uv = uv - vec2<f32>(yoked.x, -yoked.y);

    let rows = t.src.y;
    let y = uv.y * rows - 0.5;
    let h = max(abs(dpdx(y)) + abs(dpdy(y)), 1.0e-3);

    let cover = glass(q, px.y);
    if (cover <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    // Down the screen: every line near this pixel, each spread by its own
    // spot, averaged over the pixel's height. Red and blue land a little
    // off green, more so toward the edges.
    let conv = t.geom.z;
    let off_r = conv * (vec2<f32>(0.5, 0.25) + vec2<f32>(1.5, 0.75) * q);
    let off_b = conv * (vec2<f32>(-0.4, -0.3) - vec2<f32>(1.3, 0.6) * q);
    let sample_w = 1.0 / t.src.x;
    let lo = y - 0.5 * h;
    let hi = y + 0.5 * h;
    let reach = 2.5 * widest_spot() + abs(off_r.y) + abs(off_b.y);
    // A pixel much shorter than a line can take its profile at its center.
    let fine = h < 0.35;
    let first = i32(floor(lo - reach));
    let last = i32(ceil(hi + reach));
    var light = vec3<f32>(0.0);
    if (last - first > MAX_ROWS) {
        light = textureSampleLevel(glow, samp, uv, 0.0).rgb;
    } else {
        for (var r = max(first, 0); r <= min(last, i32(rows) - 1); r = r + 1) {
            let v = (f32(r) + 0.5) / rows;
            var b = textureSampleLevel(beam, samp, vec2<f32>(uv.x, v), 0.0).rgb;
            let s = textureSampleLevel(sigma, samp, vec2<f32>(uv.x, v), 0.0).rgb;
            var center = vec3<f32>(f32(r));
            if (conv > 0.0) {
                b.r = textureSampleLevel(beam, samp, vec2<f32>(uv.x - off_r.x * sample_w, v), 0.0).r;
                b.b = textureSampleLevel(beam, samp, vec2<f32>(uv.x - off_b.x * sample_w, v), 0.0).b;
                center = center + vec3<f32>(off_r.y, 0.0, off_b.y);
            }
            if (fine) {
                let d = (y - center) / s;
                light = light + b * exp(-0.5 * d * d) * 0.39894228 / s;
            } else {
                light = light + b * (phi((hi - center) / s) - phi((lo - center) / s)) / h;
            }
        }
    }
    // Blanking around the raster.
    let inside = step(vec2<f32>(0.0), uv) * step(uv, vec2<f32>(1.0));
    light = light * inside.x * inside.y;

    if (color && magnetic()) {
        light = land(light, purity(q));
    }
    let halo = textureSampleLevel(glow, samp, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
    var c = mix(light * mask(q, px), halo * inside.x * inside.y, t.glass.x);
    if (color && t.mask.x < 0.5) {
        // An aperture grille's damper wires, a third of the way in from top and bottom.
        let wire = min(abs(abs(q.y) - 0.333) / px.y, 1.0);
        c = c * mix(0.75, 1.0, wire);
    }
    // Corners are dimmer: the beam lands at an angle through thicker glass.
    c = c * (1.0 - (0.04 + 0.1 * t.geom.x) * dot(q, q)) * on.z;
    c = (c + room(q)) * cover;
    let dither = (hash(vec2<u32>(in.pos.xy)) - 0.5) / 255.0;
    return vec4<f32>(pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)) + dither, 1.0);
}
