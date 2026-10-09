//! Python `str` semantics the ported modules rely on, so results match the Python version
//! character for character: `isspace` / `strip` / `split()`, `isprintable`, `casefold`,
//! `unicodedata.category` Cc/Cf, `isdecimal`, `int(text)`, `decimal.Decimal(text)`,
//! `format(n, ",")` and `repr(float)`.
//!
//! The Unicode data comes from the Python interpreter the oracle runs on (`pystr_tables.rs`,
//! regenerated with `rust/tools/make_parity_contacts.py --tables`), not from Rust's own tables,
//! which may follow a different Unicode version.

use std::cmp::Ordering;
use std::fmt;

use crate::pystr_tables::{CASEFOLD_MULTI, CASEFOLD_RUNS, DECIMAL, FORMAT, NONPRINTABLE, SPACE};

/// Python refuses to convert integer strings longer than this (`sys.get_int_max_str_digits()`).
pub const INT_MAX_STR_DIGITS: usize = 4300;

fn in_ranges(ranges: &[(u32, u32)], ch: char) -> bool {
    let cp = ch as u32;
    ranges
        .binary_search_by(|&(first, last)| {
            if last < cp {
                Ordering::Less
            } else if first > cp {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// `ch.isspace()` — also true for the ASCII separators U+001C–U+001F, unlike `char::is_whitespace`.
pub fn is_space(ch: char) -> bool {
    in_ranges(SPACE, ch)
}

/// `ch.isprintable()`.
pub fn is_printable(ch: char) -> bool {
    !in_ranges(NONPRINTABLE, ch)
}

/// `unicodedata.category(ch) in ("Cc", "Cf")`.
pub fn is_control_or_format(ch: char) -> bool {
    matches!(ch as u32, 0x00..=0x1F | 0x7F..=0x9F) || in_ranges(FORMAT, ch)
}

/// `unicodedata.decimal(ch, None)`.
pub fn decimal_value(ch: char) -> Option<u32> {
    let cp = ch as u32;
    let at = DECIMAL.partition_point(|&(first, _)| first <= cp);
    let &(first, last) = DECIMAL.get(at.checked_sub(1)?)?;
    (cp <= last).then_some((cp - first) % 10)
}

/// `text.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `text.split()`: runs of whitespace separate, no empty items.
pub fn split(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_space).filter(|part| !part.is_empty())
}

/// The characters `str.splitlines()` breaks at ("\r\n" counts as one break).
fn is_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `text.splitlines()`: the lines without their ends.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if !is_line_break(ch) {
            continue;
        }
        lines.push(&text[start..at]);
        start = at + ch.len_utf8();
        if ch == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn casefold_char(ch: char, out: &mut String) {
    let cp = ch as u32;
    if let Ok(i) = CASEFOLD_MULTI.binary_search_by_key(&cp, |&(from, _)| from) {
        out.push_str(CASEFOLD_MULTI[i].1);
        return;
    }
    let at = CASEFOLD_RUNS.partition_point(|&(first, ..)| first <= cp);
    let mapped = at
        .checked_sub(1)
        .map(|i| CASEFOLD_RUNS[i])
        .filter(|&(first, last, stride, _)| cp <= last && (cp - first) % stride == 0)
        .and_then(|(.., delta)| cp.checked_add_signed(delta))
        .and_then(char::from_u32);
    out.push(mapped.unwrap_or(ch));
}

/// `text.casefold()` (full Unicode case folding: "Straße" → "strasse", "Σ"/"ς" → "σ").
pub fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        casefold_char(ch, &mut out);
    }
    out
}

/// A Python `int`: sign and decimal digits, of any size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyInt {
    negative: bool,
    /// No leading zeros; "0" for zero.
    digits: String,
}

/// The whitespace `int()` and `float()` skip: ASCII `isspace` (not U+001C–U+001F) and any
/// non-ASCII `str.isspace()` character.
fn is_number_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r') || (!ch.is_ascii() && is_space(ch))
}

/// ASCII form of a number literal as CPython sees it: other decimal digits become ASCII digits,
/// anything else non-ASCII makes it invalid.
fn ascii_number(text: &str) -> Option<String> {
    let text = text.trim_matches(is_number_space);
    text.chars()
        .map(|ch| match ch {
            _ if ch.is_ascii() => Some(ch),
            _ => decimal_value(ch).and_then(|d| char::from_digit(d, 10)),
        })
        .collect()
}

/// Underscores are allowed only between two digits.
fn underscores_ok(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(i, &b)| {
        b != b'_'
            || (i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
    })
}

impl PyInt {
    /// `int(text)`; `None` where Python raises `ValueError`.
    pub fn parse(text: &str) -> Option<PyInt> {
        let ascii = ascii_number(text)?;
        let (negative, body) = match ascii.as_bytes().first() {
            Some(b'-') => (true, &ascii[1..]),
            Some(b'+') => (false, &ascii[1..]),
            _ => (false, &ascii[..]),
        };
        if body.is_empty()
            || !body.bytes().all(|b| b.is_ascii_digit() || b == b'_')
            || !underscores_ok(body)
        {
            return None;
        }
        let all_digits: String = body.chars().filter(|&c| c != '_').collect();
        if all_digits.len() > INT_MAX_STR_DIGITS {
            return None;
        }
        let digits = all_digits.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        Some(PyInt {
            negative: negative && digits != "0",
            digits: digits.to_string(),
        })
    }

    pub fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    pub fn to_i64(&self) -> Option<i64> {
        let magnitude: i128 = self.digits.parse().ok()?;
        i64::try_from(if self.negative { -magnitude } else { magnitude }).ok()
    }

    /// Compare with a machine integer without overflowing.
    pub fn cmp_i64(&self, other: i64) -> Ordering {
        match self.to_i64() {
            Some(value) => value.cmp(&other),
            None if self.negative => Ordering::Less,
            None => Ordering::Greater,
        }
    }

    /// `format(n, ",")`.
    pub fn thousands(&self) -> String {
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{}", group_digits(&self.digits))
    }
}

impl From<u64> for PyInt {
    fn from(value: u64) -> Self {
        PyInt {
            negative: false,
            digits: value.to_string(),
        }
    }
}

impl fmt::Display for PyInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&self.digits)
    }
}

fn group_digits(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `format(n, ",")` for a non-negative machine integer.
pub fn thousands(n: u64) -> String {
    group_digits(&n.to_string())
}

/// `float(text)`; `None` where Python raises `ValueError`.
pub fn parse_float(text: &str) -> Option<f64> {
    let ascii = ascii_number(text)?;
    if !underscores_ok(&ascii) {
        return None;
    }
    // Rust's grammar matches Python's here: optional sign, digits with an optional point and
    // exponent, or inf / infinity / nan in any case.
    ascii.replace('_', "").parse().ok()
}

/// `repr(x)` for a float: the shortest digits that round-trip, in Python's layout
/// ("1.0", "0.0001", "1e-05", "1e+16").
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    let sci = format!("{:e}", x.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exponent.parse::<i64>().unwrap_or(0) + 1; // digits before the decimal point
    let body = if !(-3..=16).contains(&point) {
        let (lead, rest) = digits.split_at(1);
        let frac = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exp = point - 1;
        format!(
            "{lead}{frac}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    } else if point <= 0 {
        format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
    } else {
        let point = point as usize;
        if point >= digits.len() {
            format!("{digits}{}.0", "0".repeat(point - digits.len()))
        } else {
            format!("{}.{}", &digits[..point], &digits[point..])
        }
    };
    format!("{sign}{body}")
}

/// `repr(text)` for a str: single quotes unless the text has a single quote and no double
/// quote; backslash escapes for the quote, `\\`, `\t`, `\n`, `\r` and non-printable characters.
pub fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if ch == quote => {
                out.push('\\');
                out.push(ch);
            }
            _ if is_printable(ch) => out.push(ch),
            _ => out.push_str(&escape_code_point(ch as u32)),
        }
    }
    out.push(quote);
    out
}

fn escape_code_point(cp: u32) -> String {
    if cp < 0x100 {
        format!("\\x{cp:02x}")
    } else if cp < 0x1_0000 {
        format!("\\u{cp:04x}")
    } else {
        format!("\\U{cp:08x}")
    }
}

/// `text.isdecimal()`.
pub fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|ch| decimal_value(ch).is_some())
}

/// `int(text)` of a `text.isdecimal()` string, saturating at `usize::MAX` (a number that large is
/// outside every range it gets checked against); `None` if the text is not decimal.
pub fn decimal_usize(text: &str) -> Option<usize> {
    text.chars().try_fold(None, |acc: Option<usize>, ch| {
        let digit = decimal_value(ch)? as usize;
        Some(Some(
            acc.unwrap_or(0).saturating_mul(10).saturating_add(digit),
        ))
    })?
}

/// Exponent limits of CPython's exact `Decimal(text)` conversion (libmpdec's maximum context).
/// The largest exponent CPython's `Decimal` accepts (`MAX_EMAX`).
pub const DECIMAL_EMAX: i64 = 999_999_999_999_999_999;
const DECIMAL_ETINY: i64 = -1_999_999_999_999_999_997;

/// A number as `decimal.Decimal(text)` reads it.
#[derive(Clone, Debug)]
pub enum PyDecimal {
    /// `±digits × 10^exponent`, normalized: no leading or trailing zeros in `digits`. Zero is ""
    /// with a positive sign and the exponent as written (`Decimal`'s exponent limits still apply
    /// to it). `==` is numeric equality, as for `Decimal`.
    Finite {
        negative: bool,
        digits: String,
        exponent: i64,
    },
    /// Infinity or NaN: accepted by `Decimal`, but never a usable value here.
    NotFinite,
}

impl PartialEq for PyDecimal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                PyDecimal::Finite {
                    negative: a_negative,
                    digits: a_digits,
                    exponent: a_exponent,
                },
                PyDecimal::Finite {
                    negative: b_negative,
                    digits: b_digits,
                    exponent: b_exponent,
                },
            ) => {
                a_digits == b_digits
                    && (a_digits.is_empty() || (a_negative, a_exponent) == (b_negative, b_exponent))
            }
            (PyDecimal::NotFinite, PyDecimal::NotFinite) => true,
            _ => false,
        }
    }
}

impl Eq for PyDecimal {}

impl PyDecimal {
    /// `Decimal(text)` as CPython's C implementation parses it; `None` where it raises
    /// `InvalidOperation`. Surrounding whitespace is ignored, underscores anywhere are dropped,
    /// and every Unicode decimal digit counts as a digit.
    pub fn parse(text: &str) -> Option<PyDecimal> {
        let ascii: String = strip(text)
            .chars()
            .filter(|&ch| ch != '_')
            .map(|ch| match ch {
                '\x01'..='\x7f' => Some(ch),
                _ if is_space(ch) => Some(' '), // inside the number: a syntax error below
                _ => decimal_value(ch).and_then(|d| char::from_digit(d, 10)),
            })
            .collect::<Option<_>>()?;
        let (negative, body) = match ascii.as_bytes().first() {
            Some(b'-') => (true, &ascii[1..]),
            Some(b'+') => (false, &ascii[1..]),
            _ => (false, &ascii[..]),
        };
        let lower = body.to_ascii_lowercase();
        let nan_payload = lower.strip_prefix("snan").or(lower.strip_prefix("nan"));
        if matches!(lower.as_str(), "inf" | "infinity")
            || nan_payload.is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit()))
        {
            return Some(PyDecimal::NotFinite);
        }
        Self::finite(negative, body)
    }

    fn finite(negative: bool, body: &str) -> Option<PyDecimal> {
        let (mantissa, exponent) = match body.find(['e', 'E']) {
            Some(at) => (&body[..at], Some(&body[at + 1..])),
            None => (body, None),
        };
        let (int_part, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if int_part.is_empty() && frac.is_empty() || !all_digits(int_part) || !all_digits(frac) {
            return None;
        }
        let exponent = match exponent {
            None => 0,
            Some(e) => {
                let unsigned = e.strip_prefix(['+', '-']).unwrap_or(e);
                if unsigned.is_empty() || !all_digits(unsigned) {
                    return None;
                }
                e.parse::<i64>().ok()? // out of i64 range: Overflow/Clamped, i.e. invalid
            }
        };
        let exponent = exponent.saturating_sub(i64::try_from(frac.len()).ok()?);
        let coefficient = format!("{int_part}{frac}");
        let significant = coefficient.trim_start_matches('0');
        if significant.is_empty() {
            let valid = (DECIMAL_ETINY..=DECIMAL_EMAX).contains(&exponent);
            return valid.then(|| PyDecimal::Finite {
                negative: false,
                digits: String::new(),
                exponent,
            });
        }
        let adjusted = exponent.saturating_add(i64::try_from(significant.len()).ok()? - 1);
        if adjusted > DECIMAL_EMAX || exponent < DECIMAL_ETINY {
            return None;
        }
        let digits = significant.trim_end_matches('0');
        let trailing = i64::try_from(significant.len() - digits.len()).ok()?;
        Some(PyDecimal::Finite {
            negative,
            digits: digits.to_string(),
            exponent: exponent + trailing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_like_python() {
        let finite = |negative, digits: &str, exponent| PyDecimal::Finite {
            negative,
            digits: digits.into(),
            exponent,
        };
        let parse = PyDecimal::parse;
        assert_eq!(parse(" 88.50 "), Some(finite(false, "885", -1)));
        assert_eq!(parse("8.85E1"), parse("88.5"));
        assert_eq!(parse("１２_3"), Some(finite(false, "123", 0)));
        assert_eq!(parse("_1__0_"), Some(finite(false, "1", 1)));
        assert_eq!(parse("𝟖𝟖.𝟓"), parse("88.5"));
        assert_eq!(parse("-0.000"), Some(finite(false, "", 0)));
        assert_eq!(parse("+.5"), Some(finite(false, "5", -1)));
        assert_eq!(parse("1."), Some(finite(false, "1", 0)));
        assert_eq!(parse("-1E１"), Some(finite(true, "1", 1)));
        assert_eq!(
            parse("1E99999999999999999"),
            Some(finite(false, "1", 99_999_999_999_999_999))
        );
        // The limits of CPython's exact conversion, checked against Python 3.13.
        assert_eq!(
            parse("1E999999999999999999"),
            Some(finite(false, "1", DECIMAL_EMAX))
        );
        assert_eq!(parse("0E-1999999999999999997"), Some(finite(false, "", 0)));
        assert_eq!(
            parse("1E-1999999999999999997"),
            Some(finite(false, "1", DECIMAL_ETINY))
        );
        assert_eq!(parse("\x1c5\x1f"), Some(finite(false, "5", 0)));
        for not_finite in ["NAN", "-Infinity", "inf", "sNaN12"] {
            assert_eq!(
                parse(not_finite),
                Some(PyDecimal::NotFinite),
                "{not_finite:?}"
            );
        }
        for bad in [
            "",
            ".",
            "1e",
            "e5",
            "1e+",
            "1 0",
            "1\u{3000}0",
            "0x10",
            "²",
            "1.2.3",
            "1e5.0",
            "nan1.5",
            "1E999999999999999999999",
            "1E1000000000000000000",
            "\0",
            "infinit",
            "0E1000000000000000000",
            "0E-1999999999999999998",
            "10E-1999999999999999998",
            "1E-1999999999999999998",
            "1E9223372036854775807",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert!(is_decimal("０9"));
        assert!(!is_decimal("²"));
        assert_eq!(decimal_usize("１２"), Some(12));
        assert_eq!(decimal_usize(&"9".repeat(40)), Some(usize::MAX));
        assert_eq!(decimal_usize(""), None);
    }

    #[test]
    fn whitespace_and_case() {
        assert!(is_space('\x1c') && !'\x1c'.is_whitespace());
        assert_eq!(strip("\u{3000} a b \x1f"), "a b");
        assert_eq!(split(" a\u{a0}b  c ").collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(
            casefold("Straße ΟΔΥΣΣΕΥΣ ﬁ İ Ꭰ"),
            "strasse οδυσσευσ fi i\u{307} Ꭰ"
        );
        assert_eq!(casefold("ꭰ"), "Ꭰ"); // Cherokee small letters fold to capitals
        assert!(is_control_or_format('\u{200b}') && !is_control_or_format(' '));
        assert!(!is_printable('\u{a0}') && is_printable(' '));
    }

    #[test]
    fn integers_like_python() {
        let value = |t: &str| PyInt::parse(t).map(|n| n.to_string());
        assert_eq!(value(" +0_012 "), Some("12".into()));
        assert_eq!(value("-0"), Some("0".into()));
        assert_eq!(value("１２３"), Some("123".into()));
        assert_eq!(value("\u{3000}5\u{3000}"), Some("5".into()));
        for bad in [
            "",
            "+",
            "_1",
            "1_",
            "1__2",
            "+_1",
            "0x10",
            "1.0",
            "\x1c5",
            "5\u{200b}",
            "- 1",
        ] {
            assert_eq!(value(bad), None, "{bad:?}");
        }
        assert!(PyInt::parse(&"1".repeat(INT_MAX_STR_DIGITS)).is_some());
        assert!(PyInt::parse(&"0".repeat(INT_MAX_STR_DIGITS + 1)).is_none());
        assert_eq!(PyInt::parse("-1234567").unwrap().thousands(), "-1,234,567");
        assert_eq!(thousands(67_108_865), "67,108,865");
        assert_eq!(
            PyInt::parse(&"9".repeat(30)).unwrap().cmp_i64(i64::MAX),
            Ordering::Greater
        );
    }

    #[test]
    fn floats_like_python() {
        for (x, text) in [
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (123.0, "123.0"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
            (1791523309.1234567, "1791523309.1234567"),
        ] {
            assert_eq!(float_repr(x), text);
        }
        assert_eq!(parse_float(" 1_0.5 "), Some(10.5));
        assert_eq!(parse_float("-Infinity").map(f64::is_infinite), Some(true));
        assert_eq!(parse_float("1__0"), None);
        assert_eq!(parse_float("abc"), None);
    }

    #[test]
    fn repr_of_strings() {
        assert_eq!(str_repr("a'b"), "\"a'b\"");
        assert_eq!(str_repr("d'e\"f"), "'d\\'e\"f'");
        assert_eq!(str_repr("x\u{a0}\u{1}\n"), "'x\\xa0\\x01\\n'");
    }
}
