//! Window-space geometry for the native viewer: letterboxing, mouse mapping and scaling.
//! Pure functions so they are tested without a window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Largest rectangle with the frame's aspect ratio that fits the window, centred.
pub fn fit(frame: (u32, u32), window: (u32, u32)) -> Rect {
    let (fw, fh) = (u64::from(frame.0), u64::from(frame.1));
    let (ww, wh) = (u64::from(window.0), u64::from(window.1));
    if fw == 0 || fh == 0 || ww == 0 || wh == 0 {
        return Rect { x: 0, y: 0, w: 0, h: 0 };
    }
    let (w, h) = if ww * fh <= wh * fw { (ww, (ww * fh / fw).max(1)) } else { ((wh * fw / fh).max(1), wh) };
    Rect { x: ((ww - w) / 2) as u32, y: ((wh - h) / 2) as u32, w: w as u32, h: h as u32 }
}

/// Maps a window position (physical pixels) to a guest pixel. Outside the image: `None`,
/// or the nearest edge pixel with `clamp` (used while dragging).
pub fn to_guest(px: f64, py: f64, rect: &Rect, frame: (u32, u32), clamp: bool) -> Option<(u32, u32)> {
    if rect.w == 0 || rect.h == 0 || frame.0 == 0 || frame.1 == 0 {
        return None;
    }
    let (rx, ry) = (px - f64::from(rect.x), py - f64::from(rect.y));
    let inside = rx >= 0.0 && ry >= 0.0 && rx < f64::from(rect.w) && ry < f64::from(rect.h);
    if !inside && !clamp {
        return None;
    }
    let gx = (rx * f64::from(frame.0) / f64::from(rect.w)).floor();
    let gy = (ry * f64::from(frame.1) / f64::from(rect.h)).floor();
    Some((gx.clamp(0.0, f64::from(frame.0 - 1)) as u32, gy.clamp(0.0, f64::from(frame.1 - 1)) as u32))
}

/// Draws an RGB24 frame scaled (nearest neighbour) into `dst` (0RGB, `dw`×`dh`) inside `rect`;
/// the rest of the window is filled with `background`.
pub fn blit(rgb: &[u8], frame: (u32, u32), dst: &mut [u32], dw: u32, dh: u32, rect: &Rect, background: u32) {
    dst.fill(background);
    let (fw, fh) = (frame.0 as usize, frame.1 as usize);
    if rect.w == 0 || rect.h == 0 || rgb.len() < fw * fh * 3 || dst.len() < (dw as usize) * (dh as usize) {
        return;
    }
    let xs: Vec<usize> = (0..rect.w as usize).map(|x| x * fw / rect.w as usize).collect();
    for y in 0..rect.h as usize {
        let sy = y * fh / rect.h as usize;
        let src_row = &rgb[sy * fw * 3..(sy + 1) * fw * 3];
        let dy = rect.y as usize + y;
        if dy >= dh as usize {
            break;
        }
        let dst_row = &mut dst[dy * dw as usize..(dy + 1) * dw as usize];
        for (x, sx) in xs.iter().enumerate() {
            let dx = rect.x as usize + x;
            if dx >= dw as usize {
                break;
            }
            let p = &src_row[sx * 3..sx * 3 + 3];
            dst_row[dx] = (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxing_keeps_aspect_and_centres() {
        assert_eq!(fit((800, 600), (1600, 1200)), Rect { x: 0, y: 0, w: 1600, h: 1200 });
        assert_eq!(fit((800, 600), (1600, 900)), Rect { x: 200, y: 0, w: 1200, h: 900 }, "window wider than the guest");
        assert_eq!(fit((800, 600), (800, 1000)), Rect { x: 0, y: 200, w: 800, h: 600 }, "window taller than the guest");
        assert_eq!(fit((0, 0), (800, 600)).w, 0);
        assert_eq!(fit((800, 600), (0, 0)).w, 0);
        let r = fit((1920, 1080), (1, 1));
        assert!(r.w >= 1 && r.h >= 1);
    }

    #[test]
    fn mouse_maps_to_guest_pixels() {
        let r = fit((800, 600), (1600, 900)); // 1200x900 at x=200
        assert_eq!(to_guest(200.0, 0.0, &r, (800, 600), false), Some((0, 0)));
        assert_eq!(to_guest(799.0 + 200.0 + 400.0, 899.0, &r, (800, 600), false), Some((799, 599)));
        assert_eq!(to_guest(100.0, 100.0, &r, (800, 600), false), None, "in the letterbox bar");
        assert_eq!(to_guest(100.0, 100.0, &r, (800, 600), true), Some((0, 66)), "clamped to the edge while dragging");
        assert_eq!(to_guest(5000.0, 5000.0, &r, (800, 600), true), Some((799, 599)));
        assert_eq!(to_guest(1.0, 1.0, &Rect { x: 0, y: 0, w: 0, h: 0 }, (800, 600), true), None);
    }

    #[test]
    fn blit_scales_and_fills_background() {
        // 2x1 frame: red, blue → 4x2 window with a 4x2 rect.
        let rgb = [255, 0, 0, 0, 0, 255];
        let mut dst = vec![0u32; 4 * 4];
        let rect = fit((2, 1), (4, 4)); // 4x2 centred vertically at y=1
        blit(&rgb, (2, 1), &mut dst, 4, 4, &rect, 0x00222222);
        assert_eq!(&dst[0..4], &[0x222222; 4], "top bar");
        assert_eq!(&dst[4..8], &[0xff0000, 0xff0000, 0x0000ff, 0x0000ff]);
        assert_eq!(&dst[8..12], &[0xff0000, 0xff0000, 0x0000ff, 0x0000ff]);
        assert_eq!(&dst[12..16], &[0x222222; 4], "bottom bar");
        // Short buffers never panic.
        blit(&rgb[..3], (2, 1), &mut dst, 4, 4, &rect, 0);
    }
}
