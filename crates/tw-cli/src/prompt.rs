//! Small interactive prompts for `terminal-windows up`. Generic over the input and output
//! streams so the behaviour (defaults, validation, re-asking) is tested without a terminal.

use std::io::{BufRead, Write};

pub struct Prompter<R: BufRead, W: Write> {
    input: R,
    output: W,
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "input closed while waiting for an answer")
}

impl<R: BufRead, W: Write> Prompter<R, W> {
    pub fn new(input: R, output: W) -> Self {
        Self { input, output }
    }

    pub fn say(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.output, "{text}")
    }

    fn read_line(&mut self) -> std::io::Result<String> {
        self.output.flush()?;
        let mut line = String::new();
        if self.input.read_line(&mut line)? == 0 {
            return Err(eof());
        }
        Ok(line.trim().to_string())
    }

    /// Numbered choice; Enter picks `default` (0-based). Re-asks until the answer is valid.
    pub fn choose(&mut self, question: &str, options: &[&str], default: usize) -> std::io::Result<usize> {
        loop {
            writeln!(self.output, "{question}")?;
            for (i, option) in options.iter().enumerate() {
                writeln!(self.output, "  {}) {option}", i + 1)?;
            }
            write!(self.output, "Choose 1-{} [{}]: ", options.len(), default + 1)?;
            let answer = self.read_line()?;
            if answer.is_empty() {
                return Ok(default);
            }
            match answer.parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => writeln!(self.output, "  Please enter a number from 1 to {}.", options.len())?,
            }
        }
    }

    /// A whole number in `min..=max`; Enter picks `default`. Accepts a trailing unit such as "8 GiB" or "8g".
    pub fn number(&mut self, question: &str, default: u64, min: u64, max: u64, unit: &str) -> std::io::Result<u64> {
        loop {
            write!(self.output, "{question} [{default} {unit}] ({min}-{max}): ")?;
            let answer = self.read_line()?;
            if answer.is_empty() {
                return Ok(default);
            }
            match parse_number(&answer) {
                Some(n) if (min..=max).contains(&n) => return Ok(n),
                Some(n) => writeln!(self.output, "  {n} is outside {min}-{max} {unit}.")?,
                None => writeln!(self.output, "  Please enter a whole number.")?,
            }
        }
    }

    /// Free text validated by `validate`; Enter picks `default`.
    pub fn text(&mut self, question: &str, default: &str, validate: impl Fn(&str) -> Result<(), String>) -> std::io::Result<String> {
        loop {
            write!(self.output, "{question} [{default}]: ")?;
            let answer = self.read_line()?;
            let value = if answer.is_empty() { default.to_string() } else { answer };
            match validate(&value) {
                Ok(()) => return Ok(value),
                Err(why) => writeln!(self.output, "  {why}")?,
            }
        }
    }
}

/// "8", "8 GiB", "8GB", "8g" all mean 8. Anything with other text, or negative or empty, is rejected.
pub fn parse_number(text: &str) -> Option<u64> {
    let t = text.trim().to_ascii_lowercase();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = t[digits.len()..].trim();
    if digits.is_empty() || !matches!(rest, "" | "g" | "gb" | "gib" | "cpu" | "cpus" | "core" | "cores") {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompter(input: &str) -> Prompter<&[u8], Vec<u8>> {
        Prompter::new(input.as_bytes(), Vec::new())
    }

    #[test]
    fn number_parsing() {
        assert_eq!(parse_number("8"), Some(8));
        assert_eq!(parse_number(" 8 GiB "), Some(8));
        assert_eq!(parse_number("16g"), Some(16));
        assert_eq!(parse_number("4 cores"), Some(4));
        for bad in ["", "abc", "-4", "8 tb", "8.5", "0x10", "8 8"] {
            assert_eq!(parse_number(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn choose_uses_default_and_reasks_on_bad_input() {
        assert_eq!(prompter("\n").choose("How?", &["auto", "custom"], 0).unwrap(), 0);
        assert_eq!(prompter("2\n").choose("How?", &["auto", "custom"], 0).unwrap(), 1);
        let mut p = prompter("9\nx\n0\n2\n");
        assert_eq!(p.choose("How?", &["auto", "custom"], 0).unwrap(), 1);
        let shown = String::from_utf8(p.output).unwrap();
        assert_eq!(shown.matches("Please enter a number from 1 to 2").count(), 3, "three rejected answers");
    }

    #[test]
    fn number_enforces_the_range_and_reasks() {
        assert_eq!(prompter("\n").number("RAM", 8, 4, 32, "GiB").unwrap(), 8);
        let mut p = prompter("2\n64\nabc\n12 GiB\n");
        assert_eq!(p.number("RAM", 8, 4, 32, "GiB").unwrap(), 12);
        let shown = String::from_utf8(p.output).unwrap();
        assert!(shown.contains("2 is outside 4-32 GiB"));
        assert!(shown.contains("64 is outside 4-32 GiB"));
        assert!(shown.contains("Please enter a whole number"));
    }

    #[test]
    fn text_validates_and_closed_input_is_an_error() {
        let ok_only_dev = |v: &str| if v == "dev" { Ok(()) } else { Err(format!("{v} is not allowed")) };
        assert_eq!(prompter("nope\n\n").text("Name", "dev", ok_only_dev).unwrap(), "dev");
        let err = prompter("").number("RAM", 8, 4, 32, "GiB").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
