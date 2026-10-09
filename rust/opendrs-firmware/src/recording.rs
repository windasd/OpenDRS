//! Download call recordings from the radio (port of `recording.py`; replaces the vendor
//! `Wav.exe`).
//!
//! * **List** ("Read List"): a binary frame `68 58 FF 01 <csum16> <len u16 LE = 2> <n u16 BE> 10`;
//!   the checksum is the 16-bit one's-complement sum of the frame (BE words, checksum field 0).
//!   The firmware's handler reads `n` as a record number and answers that record's line;
//!   Wav.exe sends n = 100, 99, 98, ...; this module asks for 1..total instead.
//! * **Record line**: `#<no>#<total>#<R|S>#<id %08d>#<length bytes>#YYYY/MM/DD HH:MM`
//!   (R = received, S = sent).
//! * **Read** ("Read *.wav"): `"Record Read"` + record number u32 BE + pages u32 BE (ignored by
//!   the radio). The radio answers with the record line then `"Record Read"`; every `"Read"` then
//!   returns 4050 bytes (450 AMBE+2 frames of 9 bytes = 9 s), ceil(length / 4050) blocks,
//!   followed by `"ChecksumR"` + u32 BE byte sum of the blocks sent.
//!
//! The audio is AMBE+2 (72 bits per 20 ms frame). Decoding needs the vendor-supplied DVSI
//! `ambep2.exe` (proprietary, Windows only — it is run, never bundled): its input is one byte
//! (0/1) per bit, MSB first; its output is 16-bit 8 kHz mono PCM, wrapped into a WAV here. On
//! other platforms there is no default decoder: the download still works and the conversion
//! reports that no decoder is configured.
//!
//! The vendor firmware does not route either command to its handlers; the record-read patched
//! image (see [`crate::fwpatch`]) does. Both commands are read-only.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::bytes::Regex;

pub use opendrs_core::radio_link::{CHECKSUM_READ, READ_REQUEST};
use opendrs_core::radio_link::{Progress, RadioError, Transport, byte_sum};

use crate::Cancelled;
use crate::iap::last_four;

mod audio;

pub use audio::{
    DECODE_TIMEOUT, MISSING_DECODER_NOTE, NO_DECODER_NOTE, Saved, decode_ambe, default_ambep2,
    free_stem, save_recording, to_bit_stream, wav_bytes, write_wav,
};

pub const RECORD_VERB: &[u8; 11] = b"Record Read";
pub const RECORD_COMMAND_SIZE: usize = RECORD_VERB.len() + 8;
pub const BLOCK: usize = 4050;
/// One AMBE+2 frame (72 bits with FEC) = 20 ms.
pub const FRAME_BYTES: usize = 9;
pub const SAMPLE_RATE: u32 = 8000;
pub const LIST_CMD: u8 = 0x58;
pub const DEFAULT_LIST_MAX: usize = 100;
/// Wav.exe's grid limit.
pub const MAX_RECORDS: usize = 200;

pub const LIST_TIMEOUT: Duration = Duration::from_millis(1500);
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
pub const BLOCK_TIMEOUT: Duration = Duration::from_secs(6);
pub const CHECKSUM_TIMEOUT: Duration = Duration::from_secs(6);
const IDLE: Duration = Duration::from_millis(50);
const POLL: Duration = Duration::from_millis(250);

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"#([0-9]+)#([0-9]+)#([RS])#([0-9]{1,10})#([0-9]+)#([0-9]{4}/[0-9]{2}/[0-9]{2} [0-9]{2}:[0-9]{2})",
    )
    .expect("valid regex")
});

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordingError {
    /// A failure this module detected (Python: `RecordingError`).
    #[error("{0}")]
    Failed(String),
    /// A transport failure or timeout, passed through unchanged (Python: `RadioError`).
    #[error(transparent)]
    Radio(#[from] RadioError),
}

impl From<RecordingError> for RadioError {
    fn from(error: RecordingError) -> Self {
        match error {
            RecordingError::Failed(message) => RadioError::Failed(message),
            RecordingError::Radio(inner) => inner,
        }
    }
}

fn failed(message: impl Into<String>) -> RecordingError {
    RecordingError::Failed(message.into())
}

/// 16-bit one's-complement checksum (big-endian words, odd byte padded with 0).
pub fn checksum16(data: &[u8]) -> u16 {
    let mut total: u32 = 0;
    for pair in data.chunks(2) {
        let word = (u32::from(pair[0]) << 8) | pair.get(1).copied().map_or(0, u32::from);
        total += word;
        total = (total & 0xFFFF) + (total >> 16);
    }
    !(total as u16)
}

fn frame(cmd: u8, length: u16, payload: &[u8]) -> Vec<u8> {
    let mut body = [
        &[0x68, cmd, 0xFF, 0x01, 0, 0][..],
        &length.to_le_bytes(),
        payload,
        &[0x10],
    ]
    .concat();
    let sum = checksum16(&body).to_be_bytes();
    body[4..6].copy_from_slice(&sum);
    body
}

/// `68 cmd FF 01 <csum16> <len u16 LE> payload 10`; `None` when the payload does not fit a u16
/// length.
pub fn h_frame(cmd: u8, payload: &[u8]) -> Option<Vec<u8>> {
    let length = u16::try_from(payload.len()).ok()?;
    Some(frame(cmd, length, payload))
}

/// The list frame asking for record `number`.
pub fn list_request(number: u16) -> Vec<u8> {
    frame(LIST_CMD, 2, &number.to_be_bytes())
}

pub fn record_read_command(number: u32, pages: u32) -> Vec<u8> {
    [
        &RECORD_VERB[..],
        &number.to_be_bytes(),
        &pages.to_be_bytes(),
    ]
    .concat()
}

/// One recording, as the radio describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordInfo {
    pub number: u64,
    pub total: u64,
    /// 'R' received, 'S' sent.
    pub direction: char,
    pub call_id: u64,
    /// Bytes of AMBE+2 data.
    pub length: u64,
    /// "YYYY/MM/DD HH:MM" as the radio reports it.
    pub time: String,
}

fn number(field: &[u8]) -> Option<u64> {
    std::str::from_utf8(field).ok()?.parse().ok()
}

impl RecordInfo {
    /// The first record line in `text`. `None` also when a number does not fit a u64.
    pub fn parse(text: &[u8]) -> Option<RecordInfo> {
        let m = LINE.captures(text)?;
        Some(RecordInfo {
            number: number(&m[1])?,
            total: number(&m[2])?,
            direction: if &m[3] == b"R" { 'R' } else { 'S' },
            call_id: number(&m[4])?,
            length: number(&m[5])?,
            time: String::from_utf8_lossy(&m[6]).into_owned(),
        })
    }

    pub fn line(&self) -> Vec<u8> {
        format!(
            "#{}#{}#{}#{:08}#{}#{}",
            self.number, self.total, self.direction, self.call_id, self.length, self.time
        )
        .into_bytes()
    }

    /// Blocks the radio sends (it always sends at least one).
    pub fn blocks(&self) -> u64 {
        self.length.div_ceil(BLOCK as u64).max(1)
    }

    /// What Wav.exe puts in the command's page field (ignored by the radio; saturates at
    /// u32::MAX).
    pub fn pages_field(&self) -> u32 {
        u32::try_from(self.length / BLOCK as u64 + 1).unwrap_or(u32::MAX)
    }

    pub fn seconds(&self) -> f64 {
        self.length as f64 / FRAME_BYTES as f64 * 0.02
    }

    /// File name: the radio's time, then Wav.exe's `<no>_<R|S><id 8 digits>`. The time keeps
    /// a later call stored under the same record number from replacing an old file.
    pub fn stem(&self) -> String {
        let when = self.time.replace(['/', ':'], "").replace(' ', "_");
        format!(
            "{when}_{}_{}{:08}",
            self.number, self.direction, self.call_id
        )
    }
}

pub struct RecordClient<'a, T: Transport + ?Sized> {
    transport: &'a mut T,
    progress: Progress<'a>,
    cancelled: Cancelled<'a>,
    pending: Vec<u8>,
    list_stopped: bool,
    pub list_timeout: Duration,
    pub reply_timeout: Duration,
    pub block_timeout: Duration,
    pub checksum_timeout: Duration,
}

impl<'a, T: Transport + ?Sized> RecordClient<'a, T> {
    pub fn new(transport: &'a mut T) -> Self {
        RecordClient {
            transport,
            progress: Box::new(|_, _, _| {}),
            cancelled: Box::new(|| false),
            pending: Vec::new(),
            list_stopped: false,
            list_timeout: LIST_TIMEOUT,
            reply_timeout: REPLY_TIMEOUT,
            block_timeout: BLOCK_TIMEOUT,
            checksum_timeout: CHECKSUM_TIMEOUT,
        }
    }

    pub fn with_progress(self, progress: impl FnMut(usize, usize, &str) + 'a) -> Self {
        RecordClient {
            progress: Box::new(progress),
            ..self
        }
    }

    pub fn with_cancel(self, cancelled: impl FnMut() -> bool + 'a) -> Self {
        RecordClient {
            cancelled: Box::new(cancelled),
            ..self
        }
    }

    /// The last list was cut short by Stop.
    pub fn list_stopped(&self) -> bool {
        self.list_stopped
    }

    fn fill(
        &mut self,
        until: impl Fn(&[u8]) -> bool,
        timeout: Duration,
    ) -> Result<bool, RecordingError> {
        let deadline = Instant::now() + timeout;
        while !until(&self.pending) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let data = self.transport.read_idle(IDLE, remaining.min(POLL))?;
            self.pending.extend(data);
        }
        Ok(true)
    }

    fn take_line(&mut self, timeout: Duration) -> Result<Option<RecordInfo>, RecordingError> {
        if !self.fill(|b| LINE.is_match(b), timeout)? {
            return Ok(None);
        }
        let Some(m) = LINE.find(&self.pending) else {
            return Ok(None);
        };
        let info = RecordInfo::parse(m.as_bytes());
        let end = m.end();
        self.pending.drain(..end);
        Ok(info)
    }

    fn take(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RecordingError> {
        if !self.fill(|b| b.len() >= n, timeout)? {
            let got = self.pending.len();
            return Err(RecordingError::Radio(RadioError::Timeout {
                message: format!("等待對講機回應逾時（預期 {n} bytes，收到 {got}）"),
                received: got,
            }));
        }
        Ok(self.pending.drain(..n).collect())
    }

    fn take_token(
        &mut self,
        token: &[u8],
        timeout: Duration,
        extra: usize,
    ) -> Result<Option<Vec<u8>>, RecordingError> {
        let ready = |b: &[u8]| {
            crate::iap::find(b, token).is_some_and(|at| b.len() >= at + token.len() + extra)
        };
        if !self.fill(ready, timeout)? {
            return Ok(None);
        }
        let Some(at) = crate::iap::find(&self.pending, token) else {
            return Ok(None);
        };
        let end = at + token.len() + extra;
        let taken = self.pending[at..end].to_vec();
        self.pending.drain(..end);
        Ok(Some(taken))
    }

    // ------------------------------------------------------------------ list

    /// Records 1..total, one list frame each. The firmware's handler reads the frame payload as
    /// the record number and answers that record's line; record 1's line also gives the total.
    pub fn list_records(&mut self, max_count: usize) -> Result<Vec<RecordInfo>, RecordingError> {
        self.transport.reset_input()?;
        self.pending.clear();
        let mut found: Vec<RecordInfo> = Vec::new();
        // at most MAX_RECORDS, so the numbers fit the u16 payload
        let mut last = u16::try_from(max_count.min(MAX_RECORDS)).unwrap_or(0);
        let mut number: u16 = 1;
        while number <= last && !(self.cancelled)() {
            let text = format!("讀取清單 {number}/{last}…");
            (self.progress)(usize::from(number - 1), usize::from(last), &text);
            self.transport.write(&list_request(number))?;
            // the first answer may take a while (Wav.exe waits about 5 s); later ones come quickly
            let wait = if found.is_empty() {
                self.reply_timeout
            } else {
                self.list_timeout
            };
            let Some(info) = self.take_line(wait)? else {
                break;
            };
            if info.number != u64::from(number) {
                break;
            }
            last = last.min(u16::try_from(info.total).unwrap_or(u16::MAX));
            found.push(info);
            number += 1;
        }
        if (self.cancelled)() && found.is_empty() {
            return Err(failed("已停止。"));
        }
        if found.is_empty() {
            return Err(failed(
                "對講機沒有回應錄音清單指令。這台對講機的韌體可能不支援從電腦讀取錄音\
                 （DR-8500UV／DH-98VU 原廠韌體不支援，需先刷入錄音讀取修補版）；也請確認連接埠與寫頻線。",
            ));
        }
        self.list_stopped = (self.cancelled)();
        let count = found.len();
        (self.progress)(count, count, &format!("共 {count} 筆錄音"));
        Ok(found)
    }

    // ------------------------------------------------------------------ read

    /// One record: the radio's line for it and its AMBE+2 data (`length` bytes). `pages` only
    /// fills the command like Wav.exe does; the radio decides the count.
    pub fn read_record(
        &mut self,
        number: u32,
        pages: u32,
    ) -> Result<(RecordInfo, Vec<u8>), RecordingError> {
        self.transport.reset_input()?;
        self.pending.clear();
        self.transport.write(&record_read_command(number, pages))?;
        let Some(header) = self.take_line(self.reply_timeout)? else {
            return Err(failed(format!(
                "對講機沒有回傳第 {number} 筆錄音的資訊（錄音可能不存在，或韌體不支援）。"
            )));
        };
        if self
            .take_token(RECORD_VERB, self.reply_timeout, 0)?
            .is_none()
        {
            return Err(failed("對講機回應不完整（缺少 Record Read），請重試。"));
        }
        if header.number != u64::from(number) {
            return Err(failed(format!(
                "要求第 {number} 筆，對講機回傳第 {} 筆，已停止。",
                header.number
            )));
        }
        let data = self.read_blocks(number, &header)?;
        let Some(tail) = self.take_token(CHECKSUM_READ, self.checksum_timeout, 4)? else {
            return Err(failed("對講機沒有回傳校驗碼，資料可能不完整，請重試。"));
        };
        if u32::from_be_bytes(last_four(&tail)) != byte_sum(&data) {
            return Err(failed(format!(
                "第 {number} 筆錄音的校驗和不符，資料可能在傳輸中損壞，請重試。"
            )));
        }
        let blocks = saturating_usize(header.blocks());
        (self.progress)(blocks, blocks, &format!("第 {number} 筆完成"));
        let keep = saturating_usize(header.length).min(data.len());
        Ok((header, data[..keep].to_vec()))
    }

    fn read_blocks(&mut self, number: u32, header: &RecordInfo) -> Result<Vec<u8>, RecordingError> {
        let blocks = header.blocks();
        let mut data = Vec::new();
        for i in 0..blocks {
            if (self.cancelled)() {
                return Err(failed("已停止。"));
            }
            let text = format!("第 {number} 筆：{}/{blocks}", i + 1);
            (self.progress)(saturating_usize(i), saturating_usize(blocks), &text);
            self.transport.write(READ_REQUEST)?;
            data.extend(self.take(BLOCK, self.block_timeout)?);
        }
        Ok(data)
    }
}

fn saturating_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
