//! Minimal RFC 4180 reader (quoted fields, `""` escapes, newlines inside quotes), streaming.

use std::io::{self, BufRead};

pub struct CsvReader<R: BufRead> {
    r: R,
    line: String,
}

impl<R: BufRead> CsvReader<R> {
    pub fn new(r: R) -> Self {
        CsvReader { r, line: String::new() }
    }

    /// Reads the next record into `out` (cleared first). Returns `false` at end of input.
    pub fn next_record(&mut self, out: &mut Vec<String>) -> io::Result<bool> {
        out.clear();
        self.line.clear();
        if self.r.read_line(&mut self.line)? == 0 {
            return Ok(false);
        }
        let mut field = String::new();
        let mut in_quotes = false;
        loop {
            let mut chars = self.line.chars().peekable();
            while let Some(ch) = chars.next() {
                if in_quotes {
                    if ch == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            in_quotes = false;
                        }
                    } else {
                        field.push(ch);
                    }
                } else {
                    match ch {
                        '"' => in_quotes = true,
                        ',' => out.push(std::mem::take(&mut field)),
                        '\r' | '\n' => {}
                        _ => field.push(ch),
                    }
                }
            }
            if !in_quotes {
                break;
            }
            // Quoted field continues on the next line.
            self.line.clear();
            if self.r.read_line(&mut self.line)? == 0 {
                break;
            }
        }
        out.push(field);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::CsvReader;

    #[test]
    fn quotes_commas_and_newlines() {
        let data = "a,\"b, c\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",x,\n";
        let mut r = CsvReader::new(data.as_bytes());
        let mut rec = Vec::new();
        assert!(r.next_record(&mut rec).unwrap());
        assert_eq!(rec, ["a", "b, c", "say \"hi\""]);
        assert!(r.next_record(&mut rec).unwrap());
        assert_eq!(rec, ["multi\nline", "x", ""]);
        assert!(!r.next_record(&mut rec).unwrap());
    }
}
