//! Adapted from upstream terminal-mac `Sources/TerminalUI/TerminalMetrics.swift`
//! (`ImageLayout`, Copyright (c) 2026 Govijr, MIT). Logic preserved; ported to Rust.

#[derive(Debug, Clone, PartialEq)]
pub struct ImageLayout {
    /// 1-based top-left cell.
    pub column: u32,
    pub row: u32,
    pub columns: u32,
    pub rows: u32,
    pub pixel_width: f64,
    pub pixel_height: f64,
    pub cell_width: f64,
    pub cell_height: f64,
    pub width: u32,
    pub height: u32,
    pub height_limited: bool,
}

impl ImageLayout {
    /// Fits an image into the whole terminal, keeping the bottom two rows for status text.
    pub fn fullscreen(width: u32, height: u32, columns: u32, rows: u32, cell_width: f64, cell_height: f64) -> Self {
        Self::fit(width, height, 1, 1, columns, rows.saturating_sub(2), cell_width, cell_height)
    }

    /// Fits and centres the image inside the region of cells starting at (column, row).
    #[allow(clippy::too_many_arguments)]
    pub fn fit(
        width: u32,
        height: u32,
        origin_column: u32,
        origin_row: u32,
        columns: u32,
        rows: u32,
        cell_width: f64,
        cell_height: f64,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let avail_c = columns.max(1);
        let avail_r = rows.max(1);
        let height_limited = f64::from(avail_r) * cell_height / f64::from(height) <= f64::from(avail_c) * cell_width / f64::from(width);
        let (cols, rws, pw, ph);
        if height_limited {
            rws = avail_r;
            ph = f64::from(avail_r) * cell_height;
            pw = ph * f64::from(width) / f64::from(height);
            cols = ((pw / cell_width).ceil() as u32).max(1);
        } else {
            cols = avail_c;
            pw = f64::from(avail_c) * cell_width;
            ph = pw * f64::from(height) / f64::from(width);
            rws = ((ph / cell_height).ceil() as u32).max(1);
        }
        let column = (i64::from(origin_column) + (i64::from(avail_c) - i64::from(cols)) / 2).max(1) as u32;
        let row = (i64::from(origin_row) + (i64::from(avail_r) - i64::from(rws)) / 2).max(1) as u32;
        Self {
            column,
            row,
            columns: cols,
            rows: rws,
            pixel_width: pw,
            pixel_height: ph,
            cell_width,
            cell_height,
            width,
            height,
            height_limited,
        }
    }

    /// Maps a 1-based cell to a guest pixel (cell centre). `None` if outside the image unless `clamp`.
    pub fn point(&self, column: u32, row: u32, clamp: bool) -> Option<(u32, u32)> {
        let x = (f64::from(column) - f64::from(self.column) + 0.5) * self.cell_width;
        let y = (f64::from(row) - f64::from(self.row) + 0.5) * self.cell_height;
        let inside = x >= 0.0 && y >= 0.0 && x < self.pixel_width && y < self.pixel_height;
        if !clamp && !inside {
            return None;
        }
        let gx = (x * f64::from(self.width) / self.pixel_width) as i64;
        let gy = (y * f64::from(self.height) / self.pixel_height) as i64;
        Some((gx.clamp(0, i64::from(self.width) - 1) as u32, gy.clamp(0, i64::from(self.height) - 1) as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminal_is_height_limited_and_centred() {
        let l = ImageLayout::fullscreen(1920, 1080, 200, 52, 10.0, 20.0);
        assert!(l.height_limited);
        assert_eq!(l.rows, 50);
        assert_eq!(l.pixel_height, 1000.0);
        assert_eq!(l.columns, 178);
        assert_eq!(l.column, 12);
        assert_eq!(l.row, 1);
    }

    #[test]
    fn cells_map_back_to_guest_pixels() {
        let l = ImageLayout::fit(1000, 500, 1, 1, 100, 50, 10.0, 20.0);
        // Width-limited: 25 rows tall, vertically centred starting at row 13.
        assert!(!l.height_limited);
        assert_eq!((l.row, l.rows), (13, 25));
        assert_eq!(l.point(1, 13, false), Some((5, 10)));
        assert_eq!(l.point(100, 37, false), Some((995, 490)));
        assert_eq!(l.point(1, 1, false), None);
        assert_eq!(l.point(101, 13, false), None);
        assert_eq!(l.point(500, 500, true), Some((999, 499)));
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        let l = ImageLayout::fullscreen(0, 0, 0, 0, 8.0, 16.0);
        assert!(l.columns >= 1 && l.rows >= 1);
    }
}
