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
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'$' || (allow_dot && b == b'.'))
}

/// Builds an ISO 9660 image that contains `content` as `/<file_name>;1`.
/// `volume` and `file_name` must be upper-case d-characters (`A-Z 0-9 _`, plus `.` in the file name; `$` is also accepted, a deliberate deviation so the
/// Windows Setup folder `$WINPEDRIVER$` can be named).
#[cfg(test)]
pub fn single_file_iso(volume: &str, file_name: &str, content: &[u8]) -> Result<Vec<u8>, String> {
    build(volume, &[(file_name, content)])
}

/// Builds an image from `files`: `(path, content)` where a path is `NAME.EXT` in the root or
/// `DIR/NAME.EXT` in a directory one level down. All names are upper-case d-characters
/// (`A-Z 0-9 _`, plus `.` in file names), at most 28 characters.
pub fn build(volume: &str, files: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    if volume.len() > 32 || !valid_d_chars(volume, false) {
        return Err(format!("invalid ISO volume name {volume:?}"));
    }
    // Group by directory; the root is "".
    let mut root_files: Vec<(String, &[u8])> = Vec::new();
    let mut dirs: std::collections::BTreeMap<String, Vec<(String, &[u8])>> = std::collections::BTreeMap::new();
    for (path, content) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let check = |name: &str, dot: bool| -> Result<(), String> {
            if name.len() > 28 || !valid_d_chars(name, dot) {
                return Err(format!("invalid ISO name {name:?} in {path:?}"));
            }
            Ok(())
        };
        match parts.as_slice() {
            [file] => {
                check(file, true)?;
                root_files.push(((*file).to_string(), content));
            }
            [dir, file] => {
                check(dir, false)?;
                check(file, true)?;
                dirs.entry((*dir).to_string()).or_default().push(((*file).to_string(), content));
            }
            _ => return Err(format!("{path:?}: only one directory level is supported")),
        }
    }
    root_files.sort_by(|a, b| a.0.cmp(&b.0));
    for list in dirs.values_mut() {
        list.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let dir_names: Vec<String> = dirs.keys().cloned().collect(); // sorted
                                                                 // Sector plan: root dir, then one sector per directory, then file data.
    let dir_sector = |index: usize| ROOT + 1 + index;
    let mut next_data = ROOT + 1 + dir_names.len();
    let mut place = |content: &[u8]| -> (usize, u32) {
        let start = next_data;
        next_data += content.len().div_ceil(SECTOR).max(1);
        (start, content.len() as u32)
    };
    let root_placed: Vec<(String, (usize, u32))> = root_files.iter().map(|(n, c)| (n.clone(), place(c))).collect();
    let dir_placed: Vec<Vec<(String, (usize, u32))>> =
        dir_names.iter().map(|d| dirs[d].iter().map(|(n, c)| (n.clone(), place(c))).collect()).collect();
    let total_sectors = next_data;
    let mut image = vec![0u8; total_sectors * SECTOR];

    // Directory extents.
    let write_dir =
        |image: &mut Vec<u8>, sector: usize, parent_sector: usize, entries: Vec<(String, usize, u32, bool)>| -> Result<(), String> {
            let mut dir = Vec::new();
            dir.extend(dir_record(sector as u32, SECTOR as u32, true, &[0]));
            dir.extend(dir_record(parent_sector as u32, SECTOR as u32, true, &[1]));
            for (name, extent, length, is_dir) in entries {
                let id = if is_dir { name } else { format!("{name};1") };
                dir.extend(dir_record(extent as u32, length, is_dir, id.as_bytes()));
            }
            if dir.len() > SECTOR {
                return Err("too many entries in one directory".into());
            }
            image[sector * SECTOR..sector * SECTOR + dir.len()].copy_from_slice(&dir);
            Ok(())
        };
    // Root: directories and files, merged and sorted by identifier.
    let mut root_entries: Vec<(String, usize, u32, bool)> = Vec::new();
    for (i, d) in dir_names.iter().enumerate() {
        root_entries.push((d.clone(), dir_sector(i), SECTOR as u32, true));
    }
    for (name, (start, len)) in &root_placed {
        root_entries.push((name.clone(), *start, *len, false));
    }
    root_entries.sort_by(|a, b| a.0.cmp(&b.0));
    write_dir(&mut image, ROOT, ROOT, root_entries)?;
    for (i, placed) in dir_placed.iter().enumerate() {
        let entries = placed.iter().map(|(n, (start, len))| (n.clone(), *start, *len, false)).collect();
        write_dir(&mut image, dir_sector(i), ROOT, entries)?;
    }

    // Path tables: root, then each directory (parent = root).
    let mut path_l = Vec::new();
    let mut path_m = Vec::new();
    let mut entry = |id: &[u8], extent: usize, parent: u16| {
        let pad = id.len() % 2;
        let mut l = vec![0u8; 8 + id.len() + pad];
        let mut m = l.clone();
        l[0] = id.len() as u8;
        m[0] = id.len() as u8;
        l[2..6].copy_from_slice(&(extent as u32).to_le_bytes());
        m[2..6].copy_from_slice(&(extent as u32).to_be_bytes());
        l[6..8].copy_from_slice(&parent.to_le_bytes());
        m[6..8].copy_from_slice(&parent.to_be_bytes());
        l[8..8 + id.len()].copy_from_slice(id);
        m[8..8 + id.len()].copy_from_slice(id);
        path_l.extend(l);
        path_m.extend(m);
    };
    entry(&[0], ROOT, 1);
    for (i, d) in dir_names.iter().enumerate() {
        entry(d.as_bytes(), dir_sector(i), 1);
    }
    if path_l.len() > SECTOR {
        return Err("too many directories".into());
    }
    image[PATH_L * SECTOR..PATH_L * SECTOR + path_l.len()].copy_from_slice(&path_l);
    image[PATH_M * SECTOR..PATH_M * SECTOR + path_m.len()].copy_from_slice(&path_m);

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
    pvd[132..140].copy_from_slice(&both_u32(path_l.len() as u32));
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
    image[PVD * SECTOR..(PVD + 1) * SECTOR].copy_from_slice(&pvd);

    // Volume descriptor set terminator.
    image[TERMINATOR * SECTOR] = 255;
    image[TERMINATOR * SECTOR + 1..TERMINATOR * SECTOR + 6].copy_from_slice(b"CD001");
    image[TERMINATOR * SECTOR + 6] = 1;

    // File data.
    let mut copy = |placed: &(usize, u32), content: &[u8]| {
        image[placed.0 * SECTOR..placed.0 * SECTOR + content.len()].copy_from_slice(content);
    };
    for ((_, placed), (_, content)) in root_placed.iter().zip(root_files.iter()) {
        copy(placed, content);
    }
    for (i, d) in dir_names.iter().enumerate() {
        for ((_, placed), (_, content)) in dir_placed[i].iter().zip(dirs[d].iter()) {
            copy(placed, content);
        }
    }
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

/// Reads `DIR/NAME` (one directory level) or `NAME` from an image made by [`build`].
pub fn read_path(image: &[u8], path: &str) -> Option<Vec<u8>> {
    let Some((dir, file)) = path.split_once('/') else { return read_root_file(image, path) };
    let pvd = image.get(PVD * SECTOR..(PVD + 1) * SECTOR)?;
    let root_extent = u32::from_le_bytes(pvd[158..162].try_into().ok()?) as usize;
    let find = |extent: usize, name: &str| -> Option<(usize, usize)> {
        let sector = image.get(extent * SECTOR..(extent + 1) * SECTOR)?;
        let mut pos = 0;
        while pos < sector.len() && sector[pos] != 0 {
            let len = sector[pos] as usize;
            let rec = sector.get(pos..pos + len)?;
            let id_len = rec[32] as usize;
            let id = std::str::from_utf8(&rec[33..33 + id_len]).ok()?;
            if id.split(';').next() == Some(name) {
                return Some((
                    u32::from_le_bytes(rec[2..6].try_into().ok()?) as usize,
                    u32::from_le_bytes(rec[10..14].try_into().ok()?) as usize,
                ));
            }
            pos += len;
        }
        None
    };
    let (dir_extent, _) = find(root_extent, dir)?;
    let (start, size) = find(dir_extent, file)?;
    image.get(start * SECTOR..start * SECTOR + size).map(<[u8]>::to_vec)
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
    fn multiple_files_and_a_directory() {
        let a = vec![1u8; 5000];
        let b = b"driver inf".to_vec();
        let c = b"root file".to_vec();
        let iso =
            build("DRIVERS", &[("NETKVM/NETKVM.SYS", &a), ("NETKVM/NETKVM.INF", &b), ("README.TXT", &c), ("AUTOUNATTEND.XML", b"<x/>")])
                .unwrap();
        assert_eq!(iso.len() % SECTOR, 0);
        assert_eq!(read_root_file(&iso, "README.TXT").unwrap(), c);
        assert_eq!(read_root_file(&iso, "AUTOUNATTEND.XML").unwrap(), b"<x/>");
        assert_eq!(read_path(&iso, "NETKVM/NETKVM.INF").unwrap(), b);
        assert_eq!(read_path(&iso, "NETKVM/NETKVM.SYS").unwrap(), a);
        assert_eq!(read_path(&iso, "NETKVM/MISSING.X"), None);
        assert_eq!(read_path(&iso, "OTHER/NETKVM.INF"), None);
    }

    #[test]
    fn nested_directories_and_bad_paths_are_rejected() {
        assert!(build("V", &[("A/B/C.TXT", b"x")]).is_err());
        assert!(build("V", &[("a/B.TXT", b"x")]).is_err());
        assert!(build("V", &[("A/b.txt", b"x")]).is_err());
        assert!(build("V", &[("", b"x")]).is_err());
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
