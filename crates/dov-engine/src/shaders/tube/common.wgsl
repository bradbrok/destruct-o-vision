// The picture tube's settings and this refresh's timing. Mirrors `TubeGpu`
// in tube.rs.
struct Tube {
    // Signal: width, rows, rows from one line of a field to the next (2
    // interlaced, 1 progressive), lines per field.
    src: vec4<f32>,
    // This refresh, in fields from the start of the current one: from, to;
    // the current field's parity; 1 to show whole fields at once.
    time: vec4<f32>,
    // First active line of the even field, in lines from its start; full
    // drive's light, which the mask can show without clipping; the
    // brightest a refresh may show; the tube's gamma.
    scan: vec4<f32>,
    // Phosphor decay per channel, in fields (to 1/e); 1 for a monochrome tube.
    decay: vec4<f32>,
    // Gun drive per channel; black level.
    drive: vec4<f32>,
    // A monochrome tube's phosphor color.
    phosphor: vec4<f32>,
    // Spot: size in rows; growth with beam current; video bandwidth in
    // samples; samples per row of height.
    spot: vec4<f32>,
    // Mask: type (0 grille, 1 shadow, 2 slot, 3 none); amount; triads across;
    // unused.
    mask: vec4<f32>,
    // Geometry: curvature; raster size against the glass (overscan);
    // convergence error; picture aspect.
    geom: vec4<f32>,
    // Glass: halation; room light; breathing; unused.
    glass: vec4<f32>,
    // Seconds since the power switched; power (0/1); seconds since degauss
    // began; the mask's magnetism.
    events: vec4<f32>,
    // The magnet's strength; unused.
    magnet: vec4<f32>,
}

@group(0) @binding(0) var<uniform> t: Tube;

const LUMA = vec3<f32>(0.299, 0.587, 0.114);

// The widest the beam spot gets, in rows.
const MAX_SPOT: f32 = 8.0;

// The beam spot's size in rows: it grows with the beam current.
fn spot_sigma(current: vec3<f32>) -> vec3<f32> {
    let grown = t.spot.x * (1.0 + t.spot.y * sqrt(clamp(current, vec3<f32>(0.0), vec3<f32>(4.0))));
    return min(grown, vec3<f32>(MAX_SPOT));
}

// The widest spot a picture driven to twice full drive gets, in rows.
fn widest_spot() -> f32 {
    return min(t.spot.x * (1.0 + 1.42 * t.spot.y), MAX_SPOT);
}
