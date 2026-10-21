//! Raster standards: the timing grid every video-rate signal lives on.
//!
//! A sample's position is a moment in time, not a texture coordinate. Each
//! standard describes the full scan including blanking, so oscillators
//! evaluated against it behave like hardware locked to (or drifting against)
//! real sync.

/// Timing of a scanned raster. All sample counts are at `sample_rate_hz`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterStd {
    pub name: &'static str,
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
    /// Color subcarrier frequency for composite encoding; 0 for none (HD).
    pub subcarrier_hz: f64,
    /// PAL: the V chroma axis alternates every line.
    pub pal: bool,
}

impl RasterStd {
    /// 525/59.94i, sampled per Rec. 601 (13.5 MHz, 858 samples/line).
    /// Field dominance and the exact active window are simplified for now.
    pub const NTSC: Self = Self {
        name: "NTSC 480i",
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
        name: "PAL 576i",
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
        name: "HD 720p60",
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
        name: "HD 1080p60",
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
                std.name
            );
            let field_lines = std.lines_per_field().floor() as u32;
            assert!(
                std.first_active_line + std.active_lines_per_field <= field_lines,
                "{}",
                std.name
            );
        }
    }
}
