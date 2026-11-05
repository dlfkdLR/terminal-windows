//! Coordinate spaces.
//!
//! The public coordinate space of every tool, CLI command and MCP call is
//! **physical pixels of the guest's primary display, origin top-left** — the
//! same pixels a full-resolution screenshot contains. Conversions to device
//! specific ranges happen only at the edge, through these functions.

/// QEMU absolute pointer axes (usb-tablet, virtio-tablet) span 0..=0x7fff.
pub const QEMU_ABS_MAX: u32 = 0x7fff;
/// `SendInput` with `MOUSEEVENTF_ABSOLUTE` normalizes to 0..=65535.
pub const SENDINPUT_ABS_MAX: u32 = 65535;

/// Maps pixel `pos` on an axis of `extent` pixels to `0..=max`, rounding to nearest.
pub fn normalize(pos: u32, extent: u32, max: u32) -> u32 {
    if extent <= 1 {
        return 0;
    }
    let pos = u64::from(pos.min(extent - 1));
    let denom = u64::from(extent - 1);
    ((pos * u64::from(max) + denom / 2) / denom) as u32
}

pub fn to_qemu_abs(pos: u32, extent: u32) -> u32 {
    normalize(pos, extent, QEMU_ABS_MAX)
}

/// A screenshot may be downscaled before it is handed to a model. This maps a
/// point in the delivered image back to display pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageScale {
    pub image_width: u32,
    pub image_height: u32,
    pub display_width: u32,
    pub display_height: u32,
}

impl ImageScale {
    pub fn identity(width: u32, height: u32) -> Self {
        Self { image_width: width, image_height: height, display_width: width, display_height: height }
    }
    pub fn is_identity(&self) -> bool {
        self.image_width == self.display_width && self.image_height == self.display_height
    }
    /// Returns `None` if the point lies outside the image.
    pub fn to_display(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        if x >= self.image_width || y >= self.image_height {
            return None;
        }
        let map = |v: u32, from: u32, to: u32| -> u32 {
            let scaled = ((u64::from(v) * 2 + 1) * u64::from(to)) / (u64::from(from) * 2);
            (scaled as u32).min(to - 1)
        };
        Some((map(x, self.image_width, self.display_width), map(y, self.image_height, self.display_height)))
    }
}

/// Largest size that fits `max_width`×`max_height` keeping the aspect ratio,
/// never upscaling.
pub fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    let scale = f64::min(f64::from(max_width) / f64::from(width), f64::from(max_height) / f64::from(height));
    (((f64::from(width) * scale).floor() as u32).max(1), ((f64::from(height) * scale).floor() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_map_to_extremes() {
        assert_eq!(to_qemu_abs(0, 1920), 0);
        assert_eq!(to_qemu_abs(1919, 1920), QEMU_ABS_MAX);
        assert_eq!(normalize(1919, 1920, SENDINPUT_ABS_MAX), SENDINPUT_ABS_MAX);
        assert_eq!(to_qemu_abs(5000, 1920), QEMU_ABS_MAX, "clamped");
        assert_eq!(to_qemu_abs(3, 1), 0);
    }

    #[test]
    fn normalization_round_trips_every_pixel() {
        let extent = 1366;
        for x in 0..extent {
            let n = to_qemu_abs(x, extent);
            let back = ((u64::from(n) * u64::from(extent - 1) + u64::from(QEMU_ABS_MAX) / 2) / u64::from(QEMU_ABS_MAX)) as u32;
            assert_eq!(back, x);
        }
    }

    #[test]
    fn downscaled_image_points_map_to_display_pixels() {
        let s = ImageScale { image_width: 1280, image_height: 720, display_width: 2560, display_height: 1440 };
        // Image pixels map to the centre of the display block they cover.
        assert_eq!(s.to_display(0, 0), Some((1, 1)));
        assert_eq!(s.to_display(640, 360), Some((1281, 721)));
        assert_eq!(s.to_display(1279, 719), Some((2559, 1439)));
        assert_eq!(s.to_display(1280, 0), None);
        assert!(ImageScale::identity(10, 10).is_identity());
        assert_eq!(ImageScale::identity(10, 10).to_display(9, 9), Some((9, 9)));
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_within(2560, 1440, 1280, 1280), (1280, 720));
        assert_eq!(fit_within(800, 600, 1280, 1280), (800, 600));
        assert_eq!(fit_within(1080, 1920, 1000, 1000), (562, 1000));
    }
}
