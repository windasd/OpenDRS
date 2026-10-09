//! Quick text messages (常用短信): 16 slots of 40 bytes at image offset 0xFAE (port of
//! `quickmsg.py`). Text uses the same code page as names.

use crate::textcodec::{NameCodec, NameEncodingError, display_name, encode_name};

pub const QUICK_MSG_OFFSET: usize = 0xFAE;
pub const QUICK_MSG_SIZE: usize = 40;
pub const QUICK_MSG_COUNT: usize = 16;
pub const QUICK_MSG_END: usize = QUICK_MSG_OFFSET + QUICK_MSG_SIZE * QUICK_MSG_COUNT;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuickMessageError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Name(#[from] NameEncodingError),
}

pub fn has_quick_messages(image: &[u8]) -> bool {
    image.len() >= QUICK_MSG_END
}

pub fn quick_messages(image: &[u8], codec: NameCodec) -> Vec<String> {
    if !has_quick_messages(image) {
        return Vec::new();
    }
    image[QUICK_MSG_OFFSET..QUICK_MSG_END]
        .chunks(QUICK_MSG_SIZE)
        .map(|m| display_name(m, codec))
        .collect()
}

/// Set message `number` (1-16).
pub fn with_quick_message(
    image: &[u8],
    number: usize,
    text: &str,
    codec: NameCodec,
) -> Result<Vec<u8>, QuickMessageError> {
    if !has_quick_messages(image) {
        return Err(QuickMessageError::Invalid(
            "設定區太短，沒有常用短信區".into(),
        ));
    }
    if !(1..=QUICK_MSG_COUNT).contains(&number) {
        return Err(QuickMessageError::Invalid(format!(
            "短信編號必須介於 1 與 {QUICK_MSG_COUNT}"
        )));
    }
    let field = encode_name(text, QUICK_MSG_SIZE, codec)?;
    let at = QUICK_MSG_OFFSET + (number - 1) * QUICK_MSG_SIZE;
    let mut out = image.to_vec();
    out[at..at + QUICK_MSG_SIZE].copy_from_slice(&field);
    Ok(out)
}
