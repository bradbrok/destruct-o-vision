//! Raster standards: the timing grid every video-rate signal lives on.
//!
//! A sample's position is a moment in time, not a texture coordinate. Each
//! standard describes the full scan including blanking, so oscillators
//! evaluated against it behave like hardware locked to (or drifting against)
//! real sync.
//!
//! Besides the broadcast standards, a raster can be any size, at any frame
//! rate (`RasterStd::progressive`): vertical video for phones, square posts,
//! 4K. Those are progressive with square pixels, and blanked in the
//! proportions of HD.

/// Timing of a scanned raster. All sample counts are at `sample_rate_hz`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterStd {
    /// The broadcast standard's name; `None` for a raster sized by hand,
    /// which `name` names by its size and rate.
    pub standard: Option<&'static str>,
    pub sample_rate_hz: f64,
    /// Total samples per line, blanking included.
    pub samples_per_line: u32,
    /// Total lines per frame, blanking included.
    pub lines_per_frame: u32,
    pub interlaced: bool,
    pub active_samples: u32,
    /// Samples from the leading edge of H sync (0H) to the first active sample.
    pub active_start_sample: u32,
    pub active_lines_per_field: u32,
    /// Whole lines from the start of a field to its first active line.
    pub first_active_line: u32,
    /// Display aspect ratio (width / height) of the active picture.
    pub display_aspect: f32,
    /// The standard's color subcarrier; 0 for rasters without one, which
    /// carry composite video with NTSC's, scaled (see `composite`).
    pub subcarrier_hz: f64,
    /// PAL: the V chroma axis alternates every line.
    pub pal: bool,
}

impl RasterStd {
    /// 525/59.94i, sampled per Rec. 601 (13.5 MHz, 858 samples/line).
    /// Field dominance and the exact active window are simplified for now.
    pub const NTSC: Self = Self {
        standard: Some("NTSC 480i"),
        sample_rate_hz: 13_500_000.0,
        samples_per_line: 858,
        lines_per_frame: 525,
        interlaced: true,
        active_samples: 720,
        active_start_sample: 122,
        active_lines_per_field: 240,
        first_active_line: 20,
        display_aspect: 4.0 / 3.0,
        subcarrier_hz: 315_000_000.0 / 88.0,
        pal: false,
    };

    /// 625/50i, sampled per Rec. 601 (13.5 MHz, 864 samples/line).
    pub const PAL: Self = Self {
        standard: Some("PAL 576i"),
        sample_rate_hz: 13_500_000.0,
        samples_per_line: 864,
        lines_per_frame: 625,
        interlaced: true,
        active_samples: 720,
        active_start_sample: 132,
        active_lines_per_field: 288,
        first_active_line: 22,
        display_aspect: 4.0 / 3.0,
        subcarrier_hz: 4_433_618.75,
        pal: true,
    };

    /// SMPTE 296M 720p60 (74.25 MHz, 1650 samples/line).
    pub const HD720P60: Self = Self {
        standard: Some("HD 720p60"),
        sample_rate_hz: 74_250_000.0,
        samples_per_line: 1650,
        lines_per_frame: 750,
        interlaced: false,
        active_samples: 1280,
        active_start_sample: 260,
        active_lines_per_field: 720,
        first_active_line: 25,
        display_aspect: 16.0 / 9.0,
        subcarrier_hz: 0.0,
        pal: false,
    };

    /// SMPTE 274M 1080p60 (148.5 MHz, 2200 samples/line).
    pub const HD1080P60: Self = Self {
        standard: Some("HD 1080p60"),
        sample_rate_hz: 148_500_000.0,
        samples_per_line: 2200,
        lines_per_frame: 1125,
        interlaced: false,
        active_samples: 1920,
        active_start_sample: 192,
        active_lines_per_field: 1080,
        first_active_line: 41,
        display_aspect: 16.0 / 9.0,
        subcarrier_hz: 0.0,
        pal: false,
    };

    pub const ALL: [Self; 4] = [Self::NTSC, Self::PAL, Self::HD720P60, Self::HD1080P60];

    /// Shortest and longest side of a raster sized by hand.
    pub const MIN_SIDE: u32 = 32;
    pub const MAX_SIDE: u32 = 4096;

    /// A progressive raster with square pixels, `width` × `height` at `rate`
    /// frames a second (sides clamped to `MIN_SIDE ..= MAX_SIDE`). 1280×720
    /// and 1920×1080 take their HD standard's timing, so at 60 they are the
    /// standard; other sizes are blanked in the same proportions as 1080p and
    /// 4K (SMPTE 274M, CTA-861), so line-locked oscillators behave alike.
    pub fn progressive(width: u32, height: u32, rate: f64) -> Self {
        let width = width.clamp(Self::MIN_SIDE, Self::MAX_SIDE);
        let height = height.clamp(Self::MIN_SIDE, Self::MAX_SIDE);
        let rate = rate.clamp(1.0, 240.0);
        let hd = [Self::HD720P60, Self::HD1080P60]
            .into_iter()
            .find(|std| std.active_size() == (width, height));
        if let Some(std) = hd.filter(|_| rate == 60.0) {
            return std;
        }
        let (samples_per_line, active_start_sample, lines_per_frame, first_active_line) = match hd {
            Some(std) => (
                std.samples_per_line,
                std.active_start_sample,
                std.lines_per_frame,
                std.first_active_line,
            ),
            None => {
                // 1080p and 4K blank 7/48 of the width (280 of 1920) and
                // 1/24 of the height (45 of 1080), and start the picture
                // 24/35 and 41/45 of the way through those.
                let h_blank = (width * 7 / 48).max(16);
                let v_blank = (height / 24).max(4);
                (
                    width + h_blank,
                    h_blank * 24 / 35,
                    height + v_blank,
                    v_blank * 41 / 45,
                )
            }
        };
        Self {
            standard: None,
            sample_rate_hz: f64::from(samples_per_line) * f64::from(lines_per_frame) * rate,
            samples_per_line,
            lines_per_frame,
            interlaced: false,
            active_samples: width,
            active_start_sample,
            active_lines_per_field: height,
            first_active_line,
            display_aspect: width as f32 / height as f32,
            subcarrier_hz: 0.0,
            pal: false,
        }
    }

    /// The standard's name, or for a raster sized by hand, its size and rate
    /// such as "1080×1920p30". `parse` reads either back.
    pub fn name(&self) -> String {
        match self.standard {
            Some(name) => name.to_string(),
            None => {
                let (width, height) = self.active_size();
                format!("{width}×{height}p{}", self.nominal_rate())
            }
        }
    }

    /// A raster by `name`, or by a size and optional rate such as "1080x1920"
    /// (60 frames a second) or "1080×1920p30". `None` for anything else, or
    /// a size outside `MIN_SIDE ..= MAX_SIDE`.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if let Some(std) = Self::ALL.into_iter().find(|std| std.standard == Some(name)) {
            return Some(std);
        }
        let (size, rate) = match name.split_once('p') {
            Some((size, rate)) => (size, rate.parse::<f64>().ok()?),
            None => (name, 60.0),
        };
        let (width, height) = size.split_once(['x', '×'])?;
        let (width, height): (u32, u32) = (width.trim().parse().ok()?, height.trim().parse().ok()?);
        let sides = Self::MIN_SIDE..=Self::MAX_SIDE;
        if !sides.contains(&width) || !sides.contains(&height) || !(1.0..=240.0).contains(&rate) {
            return None;
        }
        Some(Self::progressive(width, height, rate))
    }

    /// The field rate, to a thousandth, as it's named: 59.94, 50, 30.
    pub fn nominal_rate(&self) -> f64 {
        (self.field_rate_hz() * 1000.0).round() / 1000.0
    }

    /// Horizontal line frequency f_H.
    pub fn line_rate_hz(&self) -> f64 {
        self.sample_rate_hz / f64::from(self.samples_per_line)
    }

    pub fn line_period(&self) -> f64 {
        1.0 / self.line_rate_hz()
    }

    pub fn sample_period(&self) -> f64 {
        1.0 / self.sample_rate_hz
    }

    pub fn fields_per_frame(&self) -> u32 {
        if self.interlaced { 2 } else { 1 }
    }

    /// Lines per field; 262.5 for NTSC.
    pub fn lines_per_field(&self) -> f64 {
        f64::from(self.lines_per_frame) / f64::from(self.fields_per_frame())
    }

    /// Vertical (field) frequency f_V.
    pub fn field_rate_hz(&self) -> f64 {
        self.line_rate_hz() / self.lines_per_field()
    }

    /// Subcarrier cycles per line (227.5 for NTSC).
    pub fn subcarrier_cycles_per_line(&self) -> f64 {
        self.subcarrier_hz / self.line_rate_hz()
    }

    pub fn field_period(&self) -> f64 {
        1.0 / self.field_rate_hz()
    }

    /// Active picture size of a full (woven) frame.
    pub fn active_size(&self) -> (u32, u32) {
        (
            self.active_samples,
            self.active_lines_per_field * self.fields_per_frame(),
        )
    }

    /// 0 or 1 for interlaced standards: which set of frame rows `field` scans.
    pub fn field_parity(&self, field: u64) -> u32 {
        if self.interlaced { (field % 2) as u32 } else { 0 }
    }

    /// Lines from the start of `field` to the start of its first active line.
    ///
    /// Line timing is continuous across the frame, but interlaced fields have
    /// a fractional line count, so every odd field begins mid-line and its
    /// first whole line starts half a line later. That half-line is also what
    /// places odd-field lines between even-field lines on screen.
    pub fn first_active_line_offset(&self, field: u64) -> f64 {
        f64::from(self.first_active_line) + 0.5 * f64::from(self.field_parity(field))
    }

    /// How composite video fits on this raster. NTSC and PAL carry their
    /// own. Any other raster carries NTSC, scaled: sync and burst shrink to
    /// fit its blanking, and the subcarrier and every bandwidth keep their
    /// place relative to the picture's width, so a VCR smears a 1080×1920
    /// picture as much as an NTSC one, and it comes back in color.
    pub fn composite(&self) -> Composite {
        if self.subcarrier_hz > 0.0 {
            return Composite {
                subcarrier_hz: self.subcarrier_hz,
                sync_clock_hz: self.sample_rate_hz,
                picture_clock_hz: self.sample_rate_hz,
            };
        }
        let ntsc = Self::NTSC;
        // The clocks that put NTSC's sync, from 0H to the picture, into this
        // raster's, and NTSC's picture across this one's.
        let seconds = |samples: u32| f64::from(samples) / ntsc.sample_rate_hz;
        let sync_clock_hz = f64::from(self.active_start_sample) / seconds(ntsc.active_start_sample);
        let picture_clock_hz = f64::from(self.active_samples) / seconds(ntsc.active_samples);
        // As many cycles across the picture as NTSC, but no closer together
        // in samples, and half a cycle over per line, so the subcarrier
        // inverts from line to line as NTSC's does and a comb separates it.
        let ntsc_per_sample = ntsc.subcarrier_hz / ntsc.sample_rate_hz;
        let per_line =
            (ntsc.subcarrier_hz / picture_clock_hz).min(ntsc_per_sample) * f64::from(self.samples_per_line);
        let per_line = (per_line - 0.5).floor() + 0.5;
        // Too narrow for a burst the decoder can lock to: black and white.
        let burst_samples = BURST_SECONDS * sync_clock_hz;
        let subcarrier_hz = if per_line >= 16.0 && burst_samples >= 8.0 {
            per_line * self.line_rate_hz()
        } else {
            0.0
        };
        Composite {
            subcarrier_hz,
            sync_clock_hz,
            picture_clock_hz,
        }
    }
}

/// The color burst's length, as the composite domain's shaders draw it
/// (`BURST_US` in `common.wgsl`).
const BURST_SECONDS: f64 = 2.5e-6;

/// Composite video on a raster (see [`RasterStd::composite`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Composite {
    /// The color subcarrier; 0 for black and white.
    pub subcarrier_hz: f64,
    /// Samples a second that the standard's timing of sync and burst, in
    /// µs, is counted at.
    pub sync_clock_hz: f64,
    /// Samples a second that bandwidths in MHz, and times across the
    /// picture (a ghost's delay), are counted at.
    pub picture_clock_hz: f64,
}

impl Composite {
    /// Subcarrier cycles per line of `raster` (227.5 for NTSC).
    pub fn cycles_per_line(&self, raster: &RasterStd) -> f64 {
        self.subcarrier_hz / raster.line_rate_hz()
    }

    /// Samples of this raster that one Rec. 601 sample of NTSC's picture
    /// stretches across: how far to scale a shift counted in samples.
    pub fn sample_scale(&self) -> f64 {
        self.picture_clock_hz / RasterStd::NTSC.sample_rate_hz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_field_rates_match_the_standards() {
        let ntsc = RasterStd::NTSC;
        assert!((ntsc.line_rate_hz() - 4_500_000.0 / 286.0).abs() < 1e-9);
        assert!((ntsc.field_rate_hz() - 59.940_06).abs() < 1e-4);
        assert_eq!(ntsc.lines_per_field(), 262.5);

        assert_eq!(RasterStd::PAL.line_rate_hz(), 15_625.0);
        assert_eq!(RasterStd::PAL.field_rate_hz(), 50.0);
        assert_eq!(RasterStd::HD720P60.field_rate_hz(), 60.0);
        assert_eq!(RasterStd::HD1080P60.field_rate_hz(), 60.0);
        // The half-cycle per line that makes NTSC dot crawl.
        assert!((ntsc.subcarrier_cycles_per_line() - 227.5).abs() < 1e-9);
        assert!((RasterStd::PAL.subcarrier_cycles_per_line() - 283.7516).abs() < 1e-4);
    }

    #[test]
    fn active_window_fits_inside_the_scan() {
        for std in RasterStd::ALL {
            assert!(
                std.active_start_sample + std.active_samples <= std.samples_per_line,
                "{}",
                std.name()
            );
            let field_lines = std.lines_per_field().floor() as u32;
            assert!(
                std.first_active_line + std.active_lines_per_field <= field_lines,
                "{}",
                std.name()
            );
        }
    }

    #[test]
    fn sized_rasters_are_progressive_with_square_pixels() {
        let tall = RasterStd::progressive(1080, 1920, 30.0);
        assert_eq!(tall.active_size(), (1080, 1920));
        assert!(!tall.interlaced);
        assert_eq!(tall.display_aspect, 0.5625);
        assert!((tall.field_rate_hz() - 30.0).abs() < 1e-9);
        assert_eq!(tall.name(), "1080×1920p30");
        // Blanked like 4K, whose CTA-861 timing this gives exactly.
        let uhd = RasterStd::progressive(3840, 2160, 60.0);
        assert_eq!((uhd.samples_per_line, uhd.lines_per_frame), (4400, 2250));
        assert_eq!((uhd.active_start_sample, uhd.first_active_line), (384, 82));
        assert_eq!(uhd.sample_rate_hz, 594_000_000.0);
        // The HD sizes keep their standard's timing, and at 60 are it.
        assert_eq!(RasterStd::progressive(1920, 1080, 60.0), RasterStd::HD1080P60);
        let hd30 = RasterStd::progressive(1280, 720, 30.0);
        assert_eq!(hd30.samples_per_line, 1650);
        assert_eq!(hd30.name(), "1280×720p30");
        assert_eq!(RasterStd::progressive(10, 99_999, 60.0).active_size(), (32, 4096));
    }

    #[test]
    fn names_read_back() {
        let rasters = [
            RasterStd::NTSC,
            RasterStd::PAL,
            RasterStd::HD720P60,
            RasterStd::HD1080P60,
            RasterStd::progressive(1080, 1920, 30.0),
            RasterStd::progressive(1080, 1350, 25.0),
            RasterStd::progressive(2160, 3840, 24.0),
        ];
        for std in rasters {
            assert_eq!(RasterStd::parse(&std.name()), Some(std), "{}", std.name());
        }
        assert_eq!(
            RasterStd::parse("1080x1920"),
            Some(RasterStd::progressive(1080, 1920, 60.0))
        );
        assert_eq!(RasterStd::parse("1920x1080p60"), Some(RasterStd::HD1080P60));
        assert_eq!(
            RasterStd::parse("1080 × 1080p50"),
            Some(RasterStd::progressive(1080, 1080, 50.0))
        );
        for bad in [
            "",
            "SECAM",
            "1080",
            "1080x",
            "x1920",
            "8x8",
            "1080x9000",
            "1080x1920p0",
            "1080x1920pfast",
        ] {
            assert_eq!(RasterStd::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn composite_fits_every_raster() {
        // NTSC and PAL carry their own.
        assert_eq!(RasterStd::NTSC.composite().sample_scale(), 1.0);
        assert_eq!(RasterStd::PAL.composite().subcarrier_hz, 4_433_618.75);
        assert_eq!(RasterStd::PAL.composite().sync_clock_hz, 13_500_000.0);
        // Other rasters carry NTSC scaled: about 191 cycles across the
        // picture, half a cycle over per line, sync and burst before it.
        for std in [
            RasterStd::HD720P60,
            RasterStd::HD1080P60,
            RasterStd::progressive(1080, 1920, 30.0),
            RasterStd::progressive(2160, 3840, 24.0),
            RasterStd::progressive(4096, 4096, 60.0),
        ] {
            let c = std.composite();
            let per_line = c.cycles_per_line(&std);
            assert_eq!(per_line.fract(), 0.5, "{}", std.name());
            let across = per_line * f64::from(std.active_samples) / f64::from(std.samples_per_line);
            assert!((185.0..=195.0).contains(&across), "{}: {across}", std.name());
            let burst_end = (5.3 + 2.5) * 1e-6 * c.sync_clock_hz;
            assert!(burst_end < f64::from(std.active_start_sample), "{}", std.name());
        }
        // A narrow picture keeps at least 3.8 samples a cycle.
        let narrow = RasterStd::progressive(360, 640, 30.0);
        let c = narrow.composite();
        assert!(c.subcarrier_hz > 0.0);
        assert!(narrow.sample_rate_hz / c.subcarrier_hz >= 3.77);
        // Too little blanking for a burst: black and white.
        assert_eq!(
            RasterStd::progressive(32, 32, 60.0).composite().subcarrier_hz,
            0.0
        );
    }

    #[test]
    fn sized_rasters_fit_their_scan() {
        for (w, h) in [
            (32, 32),
            (720, 1280),
            (1080, 1080),
            (1080, 1350),
            (2160, 3840),
            (4096, 4096),
        ] {
            let std = RasterStd::progressive(w, h, 60.0);
            assert!(
                std.active_start_sample + std.active_samples <= std.samples_per_line,
                "{}",
                std.name()
            );
            assert!(std.first_active_line >= 1, "{}", std.name());
            assert!(
                std.first_active_line + std.active_lines_per_field <= std.lines_per_frame,
                "{}",
                std.name()
            );
        }
    }
}
