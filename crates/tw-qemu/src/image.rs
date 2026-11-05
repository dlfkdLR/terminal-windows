//! Minimal image helpers for QMP screendumps (PPM from older QEMU, PNG from 7.1+).

use tw_core::{TwError, TwResult};

/// Width and height from a PNG's IHDR chunk.
pub fn png_dimensions(png: &[u8]) -> TwResult<(u32, u32)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIG || &png[12..16] != b"IHDR" {
        return Err(TwError::backend("screendump is not a PNG image"));
    }
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(TwError::backend(format!("implausible screendump size {w}x{h}")));
    }
    Ok((w, h))
}

/// Decodes a binary PPM (P6, maxval 255) into RGB8.
pub fn decode_ppm(data: &[u8]) -> TwResult<(u32, u32, Vec<u8>)> {
    let bad = || TwError::backend("malformed PPM screendump");
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < data.len() && (data[i].is_ascii_whitespace() || data[i] == b'#') {
            if data[i] == b'#' {
                while i < data.len() && data[i] != b'\n' {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let start = i;
        while i < data.len() && !data[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return Err(bad());
        }
        fields.push(std::str::from_utf8(&data[start..i]).map_err(|_| bad())?.to_string());
    }
    i += 1; // single whitespace after maxval
    if fields[0] != "P6" || fields[3] != "255" {
        return Err(bad());
    }
    let w: u32 = fields[1].parse().map_err(|_| bad())?;
    let h: u32 = fields[2].parse().map_err(|_| bad())?;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(bad());
    }
    let len = (w as usize) * (h as usize) * 3;
    let pixels = data.get(i..i + len).ok_or_else(bad)?;
    Ok((w, h, pixels.to_vec()))
}

pub fn encode_png_rgb(w: u32, h: u32, rgb: &[u8]) -> TwResult<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| TwError::backend(format!("png: {e}")))?;
        writer.write_image_data(rgb).map_err(|e| TwError::backend(format!("png: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_to_png_round_trip() {
        let mut ppm = b"P6\n# qemu\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let (w, h, rgb) = decode_ppm(&ppm).unwrap();
        assert_eq!((w, h, rgb.len()), (2, 1, 6));
        let png = encode_png_rgb(w, h, &rgb).unwrap();
        assert_eq!(png_dimensions(&png).unwrap(), (2, 1));
    }

    #[test]
    fn truncated_or_wrong_images_fail() {
        assert!(decode_ppm(b"P6\n2 1\n255\n\x00").is_err());
        assert!(decode_ppm(b"P3\n1 1\n255\n0 0 0").is_err());
        assert!(png_dimensions(b"GIF89a").is_err());
    }
}
