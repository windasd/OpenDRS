//! Python's `csv` module with the default `excel` dialect, as the CSV imports and exports use it:
//! `csv.reader(io.StringIO(text, newline=""), delimiter=…)` and
//! `csv.writer(buf, lineterminator="\r\n")`.
//!
//! The reader is a transcription of CPython's state machine (`Modules/_csv.c`), quirks
//! included: an empty line is an empty row, a quote inside an unquoted field is literal text,
//! text after a closing quote is appended, end of input inside quotes ends the field, and a field
//! longer than `csv.field_size_limit()` is an error. Rows therefore get the same numbers as in the
//! Python version, which its messages quote.

/// `csv.field_size_limit()`: the longest field, in characters.
pub const FIELD_LIMIT: usize = 131_072;

/// `_csv.Error`, with Python's message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CsvError(pub String);

/// Lines as `io.StringIO(text, newline="")` yields them: each ends after `\n`, `\r\n` or a lone
/// `\r`, terminator included.
pub fn lines(text: &str) -> Lines<'_> {
    Lines { rest: text }
}

pub struct Lines<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let bytes = self.rest.as_bytes();
        let end = match bytes.iter().position(|&b| b == b'\n' || b == b'\r') {
            None => bytes.len(),
            Some(i) if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') => i + 2,
            Some(i) => i + 1,
        };
        let (line, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(line)
    }
}

/// `csv.reader(io.StringIO(text, newline=""))`.
pub fn read(text: &str) -> Reader<Lines<'_>> {
    Reader::new(lines(text))
}

/// `csv.reader(io.StringIO(text, newline=""), delimiter=delimiter)`.
pub fn read_with(text: &str, delimiter: char) -> Reader<Lines<'_>> {
    Reader::with_delimiter(lines(text), delimiter)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

/// `csv.reader(lines)`: yields rows until the input ends or an error (which ends the rows).
pub struct Reader<I> {
    lines: I,
    delimiter: char,
    state: State,
    fields: Vec<String>,
    field: String,
    field_len: usize,
    failed: bool,
}

const NEWLINE_IN_FIELD: &str =
    "new-line character seen in unquoted field - do you need to open the file with newline=''?";

impl<I> Reader<I> {
    pub fn new(lines: I) -> Self {
        Self::with_delimiter(lines, ',')
    }

    pub fn with_delimiter(lines: I, delimiter: char) -> Self {
        Reader {
            lines,
            delimiter,
            state: State::StartRecord,
            fields: Vec::new(),
            field: String::new(),
            field_len: 0,
            failed: false,
        }
    }

    fn add_char(&mut self, ch: char) -> Result<(), CsvError> {
        if self.field_len >= FIELD_LIMIT {
            return Err(CsvError(format!(
                "field larger than field limit ({FIELD_LIMIT})"
            )));
        }
        self.field_len += 1;
        self.field.push(ch);
        Ok(())
    }

    fn save_field(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    /// End of a record: `None` is the end-of-line marker CPython feeds after every line.
    fn end_record(&mut self, ch: Option<char>) {
        self.save_field();
        self.state = if ch.is_none() {
            State::StartRecord
        } else {
            State::EatCrnl
        };
    }

    fn start_field(&mut self, ch: Option<char>) -> Result<(), CsvError> {
        match ch {
            None | Some('\n' | '\r') => self.end_record(ch),
            Some('"') => self.state = State::InQuotedField,
            Some(c) if c == self.delimiter => self.save_field(),
            Some(c) => {
                self.add_char(c)?;
                self.state = State::InField;
            }
        }
        Ok(())
    }

    fn process(&mut self, ch: Option<char>) -> Result<(), CsvError> {
        let delimiter = Some(self.delimiter);
        match (self.state, ch) {
            (State::StartRecord, None) => {} // empty line: an empty row
            (State::StartRecord, Some('\n' | '\r')) => self.state = State::EatCrnl,
            (State::StartRecord | State::StartField, _) => {
                self.state = State::StartField;
                self.start_field(ch)?;
            }
            (State::InField, None | Some('\n' | '\r')) => self.end_record(ch),
            (State::InField, _) if ch == delimiter => {
                self.save_field();
                self.state = State::StartField;
            }
            (State::InField, Some(c)) => self.add_char(c)?,
            (State::InQuotedField, None) => {} // a line break inside quotes is part of the field
            (State::InQuotedField, Some('"')) => self.state = State::QuoteInQuotedField,
            (State::InQuotedField, Some(c)) => self.add_char(c)?,
            (State::QuoteInQuotedField, Some('"')) => {
                self.add_char('"')?;
                self.state = State::InQuotedField;
            }
            (State::QuoteInQuotedField, _) if ch == delimiter => {
                self.save_field();
                self.state = State::StartField;
            }
            (State::QuoteInQuotedField, None | Some('\n' | '\r')) => self.end_record(ch),
            (State::QuoteInQuotedField, Some(c)) => {
                self.add_char(c)?;
                self.state = State::InField;
            }
            (State::EatCrnl, Some('\n' | '\r')) => {}
            (State::EatCrnl, None) => self.state = State::StartRecord,
            (State::EatCrnl, Some(_)) => return Err(CsvError(NEWLINE_IN_FIELD.into())),
        }
        Ok(())
    }

    fn process_line(&mut self, line: &str) -> Result<(), CsvError> {
        for ch in line.chars() {
            self.process(Some(ch))?;
        }
        self.process(None)
    }
}

impl<'a, I: Iterator<Item = &'a str>> Iterator for Reader<I> {
    type Item = Result<Vec<String>, CsvError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        self.fields.clear();
        self.field.clear();
        self.field_len = 0;
        self.state = State::StartRecord;
        loop {
            let Some(line) = self.lines.next() else {
                // End of input inside a quoted field: keep what was read (non-strict dialect).
                if self.field_len != 0 || self.state == State::InQuotedField {
                    self.save_field();
                    return Some(Ok(std::mem::take(&mut self.fields)));
                }
                return None;
            };
            if let Err(err) = self.process_line(line) {
                self.failed = true;
                return Some(Err(err));
            }
            if self.state == State::StartRecord {
                return Some(Ok(std::mem::take(&mut self.fields)));
            }
        }
    }
}

fn write_field(field: &str, out: &mut String) {
    if field.contains([',', '"', '\r', '\n']) {
        out.push('"');
        out.push_str(&field.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(field);
    }
}

/// One row as `csv.writer(buf, lineterminator="\r\n").writerow(fields)` writes it: fields with
/// a comma, quote or line break are quoted (quotes doubled), and a row of one empty field is `""`.
pub fn write_row<S: AsRef<str>>(fields: &[S]) -> String {
    let mut out = String::new();
    if let [only] = fields {
        if only.as_ref().is_empty() {
            return "\"\"\r\n".into();
        }
    }
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_field(field.as_ref(), &mut out);
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Result<Vec<Vec<String>>, CsvError> {
        read(text).collect()
    }

    #[test]
    fn reader_quirks_match_python() {
        let cases: [(&str, &[&[&str]]); 12] = [
            ("", &[]),
            ("\r\n", &[&[]]),
            ("a\r\n\r\nb", &[&["a"], &[], &["b"]]),
            ("a\rb", &[&["a"], &["b"]]),
            ("a,\"b\r\nc\",d\n", &[&["a", "b\r\nc", "d"]]),
            ("\"abc", &[&["abc"]]),
            ("a,\"b\"c,d", &[&["a", "bc", "d"]]),
            ("a\"b,c", &[&["a\"b", "c"]]),
            ("\"a\"\"b\"", &[&["a\"b"]]),
            ("a,b\n\n", &[&["a", "b"], &[]]),
            ("\"a\" ,b", &[&["a ", "b"]]),
            ("\u{2028}a,b\u{85}c\x1c", &[&["\u{2028}a", "b\u{85}c\x1c"]]),
        ];
        for (text, expected) in cases {
            let expected: Vec<Vec<String>> = expected
                .iter()
                .map(|r| r.iter().map(|f| f.to_string()).collect())
                .collect();
            assert_eq!(rows(text).unwrap(), expected, "{text:?}");
        }
    }

    #[test]
    fn other_delimiters() {
        let rows: Vec<Vec<String>> = read_with("a;\"b;c\";d\r\ne,f\r\n", ';')
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, [vec!["a", "b;c", "d"], vec!["e,f"]]);
        let tabbed: Vec<Vec<String>> = read_with("a\tb\t\"c\"\t", '\t')
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tabbed, [vec!["a", "b", "c", ""]]);
    }

    #[test]
    fn reader_errors_match_python() {
        let err = Reader::new(["a\rb,c"].into_iter()).next().unwrap();
        assert_eq!(err.unwrap_err().0, NEWLINE_IN_FIELD);
        let long = "x".repeat(FIELD_LIMIT + 1);
        assert_eq!(
            rows(&long).unwrap_err().0,
            "field larger than field limit (131072)"
        );
        assert!(rows(&"x".repeat(FIELD_LIMIT)).is_ok());
    }

    #[test]
    fn writer_quotes_like_python() {
        let row = write_row(&["1", "a,b", "say \"hi\"", ""]);
        assert_eq!(row, "1,\"a,b\",\"say \"\"hi\"\"\",\r\n");
        let row = write_row(&[" a", "b\"c", "d,e", "f\rg", "", "\0", "x\ny", "#", "'"]);
        assert_eq!(row, " a,\"b\"\"c\",\"d,e\",\"f\rg\",,\0,\"x\ny\",#,'\r\n");
        assert_eq!(write_row(&[""]), "\"\"\r\n");
        assert_eq!(write_row::<&str>(&[]), "\r\n");
        assert_eq!(write_row(&["", ""]), ",\r\n");
    }
}
