// What a TV carries from field to field: its oscillators (tv_scan.wgsl),
// and the gain and color killer that follow the signal.
struct ScanState {
    next_t: f32,
    period: f32,
    trig: f32,
    integ: f32,
    valid: f32,
    // Automatic gain, the burst's average strength and whether color is
    // on (the killer), as they've followed the signal.
    agc: f32,
    burst: f32,
    color: f32,
}
