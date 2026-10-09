//! Fixed-width name fields and legacy text files, without silent character loss.
//!
//! Port of `textcodec.py`. Big5 and GBK use Python's own `big5` / `gbk` mappings (generated into
//! `codec_tables.rs`), not the WHATWG tables, which differ in places (ETEN extensions, HKSCS,
//! GB18030 forms, a few individual codes such as Big5 A145), so a name decodes and encodes
//! exactly as in the Python version.

use std::collections::HashMap;
use std::sync::LazyLock;

use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::canonical_combining_class;

use crate::codec_tables::{
    BIG5_DECODE_ONLY, BIG5_ENCODE, BIG5_RUNS, GBK_DECODE_ONLY, GBK_ENCODE, GBK_RUNS,
};

const PAD_BYTES: [u8; 2] = [0x00, 0xFF];

/// A double-byte codec as Python implements it: ASCII single bytes plus a table of two-byte codes.
struct DoubleByte {
    decode: HashMap<u16, char>,
    encode: HashMap<char, u16>,
}

impl DoubleByte {
    fn new(runs: &[(u16, &str)], encode_overrides: &[(char, u16)], decode_only: &[char]) -> Self {
        let mut decode = HashMap::new();
        let mut encode = HashMap::new();
        for &(start, text) in runs {
            for (code, ch) in (start..).zip(text.chars()) {
                decode.insert(code, ch);
                encode.entry(ch).or_insert(code);
            }
        }
        encode.extend(encode_overrides.iter().copied());
        for ch in decode_only {
            encode.remove(ch);
        }
        DoubleByte { decode, encode }
    }

    /// Strict decode of the whole slice.
    fn decode(&self, bytes: &[u8]) -> Option<String> {
        let mut out = String::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let lead = bytes[i];
            if lead.is_ascii() {
                out.push(char::from(lead));
                i += 1;
                continue;
            }
            let trail = *bytes.get(i + 1)?;
            out.push(*self.decode.get(&u16::from_be_bytes([lead, trail]))?);
            i += 2;
        }
        Some(out)
    }

    /// Strict encode; `None` if any character has no code.
    fn encode(&self, text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() * 2);
        for ch in text.chars() {
            if ch.is_ascii() {
                out.push(ch as u8);
            } else {
                out.extend(self.encode.get(&ch)?.to_be_bytes());
            }
        }
        Some(out)
    }
}

static BIG5: LazyLock<DoubleByte> =
    LazyLock::new(|| DoubleByte::new(BIG5_RUNS, BIG5_ENCODE, BIG5_DECODE_ONLY));
static GBK: LazyLock<DoubleByte> =
    LazyLock::new(|| DoubleByte::new(GBK_RUNS, GBK_ENCODE, GBK_DECODE_ONLY));

/// Character set used for names stored inside the codeplug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameCodec {
    Big5,
    Gbk,
    Ascii,
}

impl NameCodec {
    pub const ALL: [NameCodec; 3] = [NameCodec::Big5, NameCodec::Gbk, NameCodec::Ascii];

    pub fn value(self) -> &'static str {
        match self {
            NameCodec::Big5 => "big5",
            NameCodec::Gbk => "gbk",
            NameCodec::Ascii => "ascii",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NameCodec::Big5 => "Big5（繁體）",
            NameCodec::Gbk => "GBK（簡體）",
            NameCodec::Ascii => "ASCII",
        }
    }

    pub fn from_value(value: &str) -> Option<NameCodec> {
        NameCodec::ALL.into_iter().find(|c| c.value() == value)
    }

    /// Strict decode of the whole slice (no replacement characters).
    pub fn decode(self, bytes: &[u8]) -> Option<String> {
        match self {
            NameCodec::Ascii => bytes
                .is_ascii()
                .then(|| String::from_utf8_lossy(bytes).into_owned()),
            NameCodec::Big5 => BIG5.decode(bytes),
            NameCodec::Gbk => GBK.decode(bytes),
        }
    }

    /// Strict encode; `None` if any character is not representable.
    pub fn encode(self, text: &str) -> Option<Vec<u8>> {
        match self {
            NameCodec::Ascii => text.is_ascii().then(|| text.as_bytes().to_vec()),
            NameCodec::Big5 => BIG5.encode(text),
            NameCodec::Gbk => GBK.encode(text),
        }
    }
}

/// Raised when a name cannot be stored in its field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NameEncodingError(pub String);

pub fn strip_padding(raw: &[u8]) -> &[u8] {
    let end = raw
        .iter()
        .position(|b| PAD_BYTES.contains(b))
        .unwrap_or(raw.len());
    &raw[..end]
}

/// Decode a padded name field; `None` when the bytes are not valid in `codec`.
pub fn decode_name(raw: &[u8], codec: NameCodec) -> Option<String> {
    codec.decode(strip_padding(raw))
}

/// Encode `text` into a zero-padded field of `size` bytes, or explain why it cannot be stored.
pub fn encode_name(
    text: &str,
    size: usize,
    codec: NameCodec,
) -> Result<Vec<u8>, NameEncodingError> {
    if text.contains('\0') {
        return Err(NameEncodingError("名稱不可包含 NUL 字元".into()));
    }
    let Some(mut body) = codec.encode(text) else {
        let encodable = |ch: &char| codec.encode(ch.encode_utf8(&mut [0; 4])).is_some();
        let bad_from = text.chars().skip_while(encodable);
        // Python's ascii codec reports the whole run of unencodable characters, the CJK
        // codecs one character at a time.
        let bad: String = match codec {
            NameCodec::Ascii => bad_from.take_while(|ch| !encodable(ch)).collect(),
            NameCodec::Big5 | NameCodec::Gbk => bad_from.take(1).collect(),
        };
        return Err(NameEncodingError(format!(
            "「{bad}」無法以 {} 表示",
            codec.label()
        )));
    };
    if body.len() > size {
        return Err(NameEncodingError(format!(
            "名稱需 {} bytes，超過欄位上限 {size} bytes（中文字每字 2 bytes）",
            body.len()
        )));
    }
    body.resize(size, 0);
    Ok(body)
}

fn hex_spaced(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Human-readable form: undecodable bytes are shown as hex rather than `?`, and control
/// characters as visible symbols (CR → ␍, LF → ␊).
pub fn display_name(raw: &[u8], codec: NameCodec) -> String {
    match decode_name(raw, codec) {
        Some(text) => text
            .chars()
            .map(|ch| {
                if (ch as u32) < 0x20 {
                    char::from_u32(0x2400 + ch as u32).unwrap_or(ch)
                } else {
                    ch
                }
            })
            .collect(),
        None => format!("‹{}›", hex_spaced(strip_padding(raw))),
    }
}

/// Best-effort fit for imported names: returns `(field, stored_text)`. Accents are folded,
/// control and unrepresentable characters dropped, and the text cut at a character boundary.
pub fn fit_name(text: &str, size: usize, codec: NameCodec) -> (Vec<u8>, String) {
    let mut used = 0;
    let mut kept = String::new();
    for ch in text.nfkd() {
        if canonical_combining_class(ch) != 0 || (ch as u32) < 0x20 || ch == '\x7f' {
            continue;
        }
        let Some(encoded) = codec.encode(ch.encode_utf8(&mut [0; 4])) else {
            continue;
        };
        if used + encoded.len() > size {
            break;
        }
        used += encoded.len();
        kept.push(ch);
    }
    let stored = kept.trim_end().to_string();
    let mut body = codec.encode(&stored).unwrap_or_default();
    body.resize(size, 0);
    (body, stored)
}

// Big5 and GBK byte ranges overlap heavily: text in one usually decodes *without error* in the
// other, just as different characters. Decode with both and keep the result made of more
// everyday characters.
const COMMON_HANZI: &str = concat!(
    "的一是不了人我在有他這这中大來来上國国個个到說说們们為为子和你地出道也時时年得就那要下以",
    "生會会自著着去之過过家學学對对可裡里後后小麼么心多天而能好都然沒没日於于起還还發发成事只作",
    "當当想看文無无開开手十用主行方又如前所本見见經经頭头面公同三已老從从動动兩两長长知民樣样現现",
    "分將将外但身些與与高意進进把法此實实回二理美點点月明其種种聲声全工己話话兒儿者向情部正名定女",
    "問问力機机給给等幾几很業业最間间新什打便位因重被走電电四第門门相次東东政海口使教西再平真聽听",
    "世氣气信北少關关並并內内加化由卻却代軍军產产入先山五太水萬万市眼體体別别處处總总才場场師师書书",
    "比住員员九笑性通目華华報报立馬马命張张活難难神數数件安表原車车白應应路期叫死常提感金何更反合",
    "放做系計计或司利受光王果親亲界及今京務务制解各任至清物台臺象記记邊边共風风戰战干接條条該该達达",
    "城半跟形認认音保收術术約约連连團团必流管設设建交且區区族友指義义科類类備备頻频率碼码號号道組组",
    "聯联絡络錄录掃扫描語语檔档線线講讲單单選选項项列表訊讯息呼叫鍵键鎖锁靜静噪功池量時区域網网版",
);

fn hanzi_score(text: &str) -> f64 {
    let cjk: Vec<char> = text
        .chars()
        .filter(|ch| ('\u{3400}'..='\u{9fff}').contains(ch))
        .collect();
    if cjk.is_empty() {
        return 0.0;
    }
    cjk.iter().filter(|ch| COMMON_HANZI.contains(**ch)).count() as f64 / cjk.len() as f64
}

/// Decode `data` as Big5 or GBK, whichever yields more common characters (Big5 on a tie).
pub fn best_cjk_decoding(data: &[u8]) -> Option<(String, NameCodec)> {
    let mut best: Option<(f64, String, NameCodec)> = None;
    for codec in [NameCodec::Big5, NameCodec::Gbk] {
        if let Some(text) = codec.decode(data) {
            let score = hanzi_score(&text);
            if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                best = Some((score, text, codec));
            }
        }
    }
    best.map(|(_, text, codec)| (text, codec))
}

/// Guess the codec of a set of name fields; `None` if all are plain ASCII.
pub fn suggest_codec(fields: &[&[u8]]) -> Option<NameCodec> {
    let parts: Vec<&[u8]> = fields
        .iter()
        .filter(|f| f.iter().any(|b| *b >= 0x80))
        .map(|f| strip_padding(f))
        .collect();
    let blob = parts.join(&b' ');
    if blob.is_empty() {
        return None;
    }
    best_cjk_decoding(&blob).map(|(_, codec)| codec)
}

const CP1252_UNDEFINED: [u8; 5] = [0x81, 0x8D, 0x8F, 0x90, 0x9D];

/// Decode a legacy config/help file, returning `(text, encoding_used)`.
pub fn read_legacy_text(data: &[u8]) -> (String, &'static str) {
    let utf16 = match data {
        [0xFF, 0xFE, rest @ ..] => Some((UTF_16LE, rest)),
        [0xFE, 0xFF, rest @ ..] => Some((UTF_16BE, rest)),
        _ => None,
    };
    if let Some((encoding, rest)) = utf16 {
        let (text, had_errors) = encoding.decode_without_bom_handling(rest);
        if !had_errors {
            return (text.into_owned(), "utf-16");
        }
        // Not UTF-16 after all (e.g. a cp1252 file starting with "ÿþ"): try the others.
    }
    let body = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    if let Ok(text) = std::str::from_utf8(body) {
        return (text.to_string(), "utf-8-sig");
    }
    if let Some((text, codec)) = best_cjk_decoding(data) {
        return (text, codec.value());
    }
    if !data.iter().any(|b| CP1252_UNDEFINED.contains(b)) {
        return (
            WINDOWS_1252
                .decode_without_bom_handling(data)
                .0
                .into_owned(),
            "cp1252",
        );
    }
    (data.iter().map(|&b| b as char).collect(), "latin-1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_visible_and_bad_bytes_are_hex() {
        assert_eq!(
            display_name(b"QA0EL\r\n\0\0\0", NameCodec::Ascii),
            "QA0EL␍␊"
        );
        assert_eq!(
            display_name(b"\xa4\xa4\xff\xff", NameCodec::Ascii),
            "‹a4 a4›"
        );
        assert_eq!(
            display_name("中文".as_bytes(), NameCodec::Ascii),
            "‹e4 b8 ad e6 96 87›"
        );
    }

    #[test]
    fn encoding_errors_name_the_problem() {
        assert_eq!(
            encode_name("TG9", 10, NameCodec::Ascii).unwrap(),
            b"TG9\0\0\0\0\0\0\0"
        );
        let err = encode_name("中文", 10, NameCodec::Ascii).unwrap_err();
        assert_eq!(err.0, "「中文」無法以 ASCII 表示");
        assert!(
            encode_name("頻道一二三四五", 10, NameCodec::Big5)
                .unwrap_err()
                .0
                .contains("14 bytes")
        );
        assert!(encode_name("a\0b", 10, NameCodec::Big5).is_err());
    }
}
