//! Adapted from upstream terminal-mac `TerminalMetricFilter` (TerminalMetrics.swift,
//! Copyright (c) 2026 Govijr, MIT). Additions: DA1 (`ESC [ ? … c`) capture, used
//! to detect Sixel support (attribute 4) in Windows Terminal and others.

#[derive(Debug, Default)]
pub struct MetricFilter {
    pub cell_width: Option<f64>,
    pub cell_height: Option<f64>,
    pub window_width: Option<f64>,
    pub window_height: Option<f64>,
    /// Kitty graphics replies keyed by image id, e.g. `{31: "OK"}`.
    pub graphics_replies: std::collections::HashMap<u32, String>,
    /// Primary device attributes from a DA1 reply.
    pub device_attributes: Option<Vec<u32>>,
    pending: Vec<u8>,
}

const MAX_PENDING: usize = 4096;

impl MetricFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sixel(&self) -> bool {
        self.device_attributes.as_ref().is_some_and(|a| a.contains(&4))
    }

    /// Removes terminal replies from `bytes`; returns what should reach the input parser.
    pub fn feed(&mut self, bytes: &[u8], flush: bool) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        while !self.pending.is_empty() {
            if self.pending[0] != 27 {
                out.push(self.pending.remove(0));
                continue;
            }
            if self.pending.len() == 1 {
                if flush {
                    out.push(self.pending.remove(0));
                }
                break;
            }
            match self.pending[1] {
                b'[' => {
                    let Some(end) = (2..self.pending.len()).find(|&i| (64..=126).contains(&self.pending[i])) else { break };
                    let seq: Vec<u8> = self.pending.drain(..=end).collect();
                    if !self.csi(&seq) {
                        out.extend(seq);
                    }
                }
                b'_' => {
                    let Some(end) = (3..self.pending.len()).find(|&i| self.pending[i] == b'\\' && self.pending[i - 1] == 27) else { break };
                    let seq: Vec<u8> = self.pending.drain(..=end).collect();
                    self.apc(&seq[2..seq.len() - 2]);
                }
                _ => out.push(self.pending.remove(0)),
            }
            if self.pending.len() > MAX_PENDING {
                self.pending.clear();
            }
        }
        out
    }

    fn csi(&mut self, seq: &[u8]) -> bool {
        let body = String::from_utf8_lossy(&seq[2..seq.len() - 1]).into_owned();
        match seq[seq.len() - 1] {
            b't' => {
                let v: Vec<f64> = body.split(';').filter_map(|p| p.parse().ok()).collect();
                if v.len() == 3 && v[1] > 0.0 && v[2] > 0.0 && v[1].is_finite() && v[2].is_finite() {
                    match v[0] as u32 {
                        6 => {
                            self.cell_height = Some(v[1]);
                            self.cell_width = Some(v[2]);
                            return true;
                        }
                        4 => {
                            self.window_height = Some(v[1]);
                            self.window_width = Some(v[2]);
                            return true;
                        }
                        _ => {}
                    }
                }
                false
            }
            b'c' if body.starts_with('?') => {
                self.device_attributes = Some(body[1..].split(';').filter_map(|p| p.parse().ok()).collect());
                true
            }
            _ => false,
        }
    }

    fn apc(&mut self, body: &[u8]) {
        let text = String::from_utf8_lossy(body);
        let Some(rest) = text.strip_prefix('G') else { return };
        let Some((keys, message)) = rest.split_once(';') else { return };
        if let Some(id) = keys.split(',').find_map(|kv| kv.strip_prefix("i=")).and_then(|v| v.parse().ok()) {
            self.graphics_replies.insert(id, message.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_size_reports_kitty_and_da1_replies() {
        let mut f = MetricFilter::new();
        let input = b"a\x1b[6;20;10t\x1b[4;800;1600t\x1b_Gi=31;OK\x1b\\\x1b[?62;4;22cb\x1b[A";
        let out = f.feed(input, false);
        assert_eq!(out, b"ab\x1b[A");
        assert_eq!((f.cell_width, f.cell_height), (Some(10.0), Some(20.0)));
        assert_eq!(f.window_width, Some(1600.0));
        assert_eq!(f.graphics_replies.get(&31).map(String::as_str), Some("OK"));
        assert!(f.sixel());
    }

    #[test]
    fn split_replies_are_reassembled() {
        let mut f = MetricFilter::new();
        assert!(f.feed(b"\x1b_Gi=31;EN", false).is_empty());
        assert!(f.feed(b"OENT\x1b\\", false).is_empty());
        assert_eq!(f.graphics_replies.get(&31).map(String::as_str), Some("ENOENT"));
    }
}
