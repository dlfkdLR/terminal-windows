//! A minimal ISO 9660 writer: one file in the root directory. Used to hand Windows Setup its
//! `autounattend.xml` on a CD. A CD (not a USB stick or disk) is used on purpose: in Windows PE a
//! USB stick is a *disk*, which could shift the disk numbering that the answer file relies on.
//!
//! Layout (2048-byte sectors): 16 system sectors, primary volume descriptor, terminator,
//! little- and big-endian path tables, the root directory, then the file data.

const SECTOR: usize = 2048;
const PVD: usize = 16;
const TERMINATOR: usize = 17;
const PATH_L: usize = 18;
const PATH_M: usize = 19;
const ROOT: usize = 20;
const DATA: usize = 21;

fn both_u32(v: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&v.to_le_bytes());
    out[4..].copy_from_slice(&v.to_be_bytes());
    out
}

fn both_u16(v: u16) -> [u8; 4] {
    let mut out = [0u8; 4];
    out[..2].copy_from_slice(&v.to_le_bytes());
    out[2..].copy_from_slice(&v.to_be_bytes());
    out
}

fn padded(text: &str, len: usize) -> Vec<u8> {
    let mut v = text.as_bytes().to_vec();
    v.resize(len, b' ');
    v
}

/// A directory record. `id` is the file identifier (`[0]` is ".", `[1]` is "..").
fn dir_record(extent: u32, length: u32, is_dir: bool, id: &[u8]) -> Vec<u8> {
    let pad = usize::from(id.len().is_multiple_of(2));
    let total = 33 + id.len() + pad;
    let mut r = vec![0u8; total];
    r[0] = total as u8;
    r[2..10].copy_from_slice(&both_u32(extent));
    r[10..18].copy_from_slice(&both_u32(length));
    // Recording date: 2026-01-01 00:00:00 UTC (years since 1900).
    r[18..25].copy_from_slice(&[126, 1, 1, 0, 0, 0, 0]);
    r[25] = if is_dir { 0x02 } else { 0 };
    r[28..32].copy_from_slice(&both_u16(1));
    r[32] = id.len() as u8;
    r[33..33 + id.len()].copy_from_slice(id);
    r
}

fn valid_d_chars(s: &str, allow_dot: bool) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || (allow_dot && b == b'.'))
}

/// Builds an ISO 9660 image that contains `content` as `/<file_name>;1`.
/// `volume` and `file_name` must be upper-case d-characters (`A-Z 0-9 _`, plus `.` in the file name).
pub fn single_file_iso(volume: &str, file_name: &str, content: &[u8]) -> Result<Vec<u8>, String> {
    if volume.len() > 32 || !valid_d_chars(volume, false) {
        return Err(format!("invalid ISO volume name {volume:?}"));
    }
    if file_name.len() > 28 || !valid_d_chars(file_name, true) {
        return Err(format!("invalid ISO file name {file_name:?}"));
    }
    let length = u32::try_from(content.len()).map_err(|_| "file too large".to_string())?;
    let data_sectors = content.len().div_ceil(SECTOR).max(1);
    let total_sectors = DATA + data_sectors;
    let mut image = vec![0u8; total_sectors * SECTOR];
    let sector = |image: &mut Vec<u8>, n: usize| -> std::ops::Range<usize> {
        let _ = &image;
        n * SECTOR..(n + 1) * SECTOR
    };

    // Root directory.
    let file_id = format!("{file_name};1");
    let mut dir = Vec::new();
    dir.extend(dir_record(ROOT as u32, SECTOR as u32, true, &[0]));
    dir.extend(dir_record(ROOT as u32, SECTOR as u32, true, &[1]));
    dir.extend(dir_record(DATA as u32, length, false, file_id.as_bytes()));
    let r = sector(&mut image, ROOT);
    image[r.start..r.start + dir.len()].copy_from_slice(&dir);

    // Path tables: one entry, the root directory (10 bytes).
    let mut path = vec![0u8; 10];
    path[0] = 1;
    let (mut little, mut big) = (path.clone(), path);
    little[2..6].copy_from_slice(&(ROOT as u32).to_le_bytes());
    little[6..8].copy_from_slice(&1u16.to_le_bytes());
    big[2..6].copy_from_slice(&(ROOT as u32).to_be_bytes());
    big[6..8].copy_from_slice(&1u16.to_be_bytes());
    let r = sector(&mut image, PATH_L);
    image[r.start..r.start + 10].copy_from_slice(&little);
    let r = sector(&mut image, PATH_M);
    image[r.start..r.start + 10].copy_from_slice(&big);

    // Primary volume descriptor.
    let mut pvd = vec![0u8; SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[8..40].copy_from_slice(&padded("TERMINAL_WINDOWS", 32));
    pvd[40..72].copy_from_slice(&padded(volume, 32));
    pvd[80..88].copy_from_slice(&both_u32(total_sectors as u32));
    pvd[120..124].copy_from_slice(&both_u16(1));
    pvd[124..128].copy_from_slice(&both_u16(1));
    pvd[128..132].copy_from_slice(&both_u16(SECTOR as u16));
    pvd[132..140].copy_from_slice(&both_u32(10));
    pvd[140..144].copy_from_slice(&(PATH_L as u32).to_le_bytes());
    pvd[148..152].copy_from_slice(&(PATH_M as u32).to_be_bytes());
    pvd[156..190].copy_from_slice(&dir_record(ROOT as u32, SECTOR as u32, true, &[0]));
    for (start, len) in [(190, 128), (318, 128), (446, 128), (574, 128), (702, 37), (739, 37), (776, 37)] {
        pvd[start..start + len].fill(b' ');
    }
    for start in [813, 830, 847, 864] {
        pvd[start..start + 16].copy_from_slice(b"2026010100000000");
    }
    pvd[881] = 1;
    let r = sector(&mut image, PVD);
    image[r].copy_from_slice(&pvd);

    // Volume descriptor set terminator.
    let r = sector(&mut image, TERMINATOR);
    image[r.start] = 255;
    image[r.start + 1..r.start + 6].copy_from_slice(b"CD001");
    image[r.start + 6] = 1;

    // File data.
    image[DATA * SECTOR..DATA * SECTOR + content.len()].copy_from_slice(content);
    Ok(image)
}

/// Reads back `/<name>` from an image made by [`single_file_iso`] (used by tests and diagnostics).
pub fn read_root_file(image: &[u8], name: &str) -> Option<Vec<u8>> {
    let pvd = image.get(PVD * SECTOR..(PVD + 1) * SECTOR)?;
    if &pvd[1..6] != b"CD001" || pvd[0] != 1 {
        return None;
    }
    let root_extent = u32::from_le_bytes(pvd[158..162].try_into().ok()?) as usize;
    let dir = image.get(root_extent * SECTOR..(root_extent + 1) * SECTOR)?;
    let mut pos = 0;
    while pos < dir.len() && dir[pos] != 0 {
        let len = dir[pos] as usize;
        let rec = dir.get(pos..pos + len)?;
        let id_len = rec[32] as usize;
        let id = std::str::from_utf8(&rec[33..33 + id_len]).ok()?;
        if id.split(';').next() == Some(name) {
            let extent = u32::from_le_bytes(rec[2..6].try_into().ok()?) as usize;
            let size = u32::from_le_bytes(rec[10..14].try_into().ok()?) as usize;
            return image.get(extent * SECTOR..extent * SECTOR + size).map(<[u8]>::to_vec);
        }
        pos += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_small_and_multi_sector_files() {
        for size in [0usize, 1, 100, 2047, 2048, 2049, 10_000] {
            let content: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let iso = single_file_iso("UNATTEND", "AUTOUNATTEND.XML", &content).unwrap();
            assert_eq!(iso.len() % SECTOR, 0);
            assert_eq!(read_root_file(&iso, "AUTOUNATTEND.XML").unwrap(), content, "size {size}");
        }
    }

    #[test]
    fn descriptors_are_well_formed() {
        let iso = single_file_iso("UNATTEND", "AUTOUNATTEND.XML", b"<x/>").unwrap();
        assert_eq!(&iso[16 * SECTOR + 1..16 * SECTOR + 6], b"CD001");
        assert_eq!(iso[16 * SECTOR], 1, "primary volume descriptor");
        assert_eq!(iso[17 * SECTOR], 255, "terminator");
        let total = u32::from_le_bytes(iso[16 * SECTOR + 80..16 * SECTOR + 84].try_into().unwrap()) as usize;
        assert_eq!(total * SECTOR, iso.len(), "volume space size matches the image");
        assert_eq!(
            u32::from_le_bytes(iso[16 * SECTOR + 80..16 * SECTOR + 84].try_into().unwrap()),
            u32::from_be_bytes(iso[16 * SECTOR + 84..16 * SECTOR + 88].try_into().unwrap())
        );
        assert_eq!(&iso[16 * SECTOR + 40..16 * SECTOR + 48], b"UNATTEND");
        assert_eq!(read_root_file(&iso, "MISSING.TXT"), None);
        assert_eq!(read_root_file(&iso[..SECTOR], "AUTOUNATTEND.XML"), None, "truncated images do not panic");
    }

    #[test]
    fn rejects_names_outside_the_d_character_set() {
        assert!(single_file_iso("unattend", "A.XML", b"").is_err());
        assert!(single_file_iso("OK", "autounattend.xml", b"").is_err());
        assert!(single_file_iso("OK", "A B.XML", b"").is_err());
        assert!(single_file_iso("OK", &"A".repeat(29), b"").is_err());
        assert!(single_file_iso(&"V".repeat(33), "A.XML", b"").is_err());
    }
}
