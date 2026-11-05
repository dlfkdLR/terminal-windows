//! Unicode text → UTF-16 chunks for `SendInput(KEYEVENTF_UNICODE)`.

/// Splits `text` into UTF-16 chunks of at most `max_units`, never separating a
/// surrogate pair. Newlines become a separate `None` entry so the caller can
/// press Enter (KEYEVENTF_UNICODE with U+000A is ignored by many controls).
pub fn chunks(text: &str, max_units: usize) -> Vec<Option<Vec<u16>>> {
    assert!(max_units >= 2, "a surrogate pair needs two units");
    let mut out = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !current.is_empty() {
                out.push(Some(std::mem::take(&mut current)));
            }
            out.push(None);
            continue;
        }
        if ch == '\r' {
            continue;
        }
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        if current.len() + units.len() > max_units {
            out.push(Some(std::mem::take(&mut current)));
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        out.push(Some(current));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_surrogate_pairs_together_and_splits_lines() {
        let c = chunks("a😀b\r\n한", 2);
        let decoded: Vec<Option<String>> = c.iter().map(|x| x.as_ref().map(|u| String::from_utf16(u).unwrap())).collect();
        assert_eq!(decoded, vec![Some("a".into()), Some("😀".into()), Some("b".into()), None, Some("한".into())]);
    }

    #[test]
    fn long_text_is_chunked() {
        let c = chunks(&"x".repeat(40), 16);
        assert_eq!(c.len(), 3);
        assert_eq!(c[2].as_ref().unwrap().len(), 8);
    }
}
