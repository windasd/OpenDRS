//! Firmware upgrade through the radio's IAP bootloader (port of `iap.py`; replaces the vendor
//! `IAP+Flash.exe`).
//!
//! ```text
//! PC -> "Erase      " (11 bytes) + target address u32 BE + last page index u32 BE
//! RF -> text containing "ok" (the vendor tool waits for "ok", the radio's clone code for "IAP")
//! then for page = 0 ..= last page index:
//! PC -> 2048 bytes of the image, the last page padded with 0x00
//! RF -> "kyd"
//! after the last page:
//! RF -> "ChecksumI" + u32 BE byte sum of every page sent (padding included)
//! ```
//!
//! The last page index is `len / 2048` exactly as the vendor tool computes it, so an image that
//! is an exact multiple of 2048 gets one extra page of zeros. The byte stream was verified
//! identical to the vendor tool's (run against a simulated bootloader) and the upgrade was
//! verified on hardware on 2026-10-09 (vendor image and record-read patched image).
//!
//! The bootloader must already be running: the user puts the radio into IAP mode. Once "Erase"
//! is accepted the radio has no application until every page has arrived — an interrupted
//! upgrade leaves it in the bootloader, from where the upgrade can simply be repeated.

use std::slice::ChunksExact;
use std::time::{Duration, Instant};

use opendrs_core::radio_link::{Progress, RadioError, Transport, byte_sum, bytes_repr};

use crate::Cancelled;
use crate::pytext::group_thousands;

pub const PAGE_SIZE: usize = 2048;
pub const ERASE_VERB: &[u8; 11] = b"Erase      ";
pub const ERASE_ACK: &[u8] = b"ok";
pub const PAGE_ACK: &[u8] = b"kyd";
pub const CHECKSUM_TAG: &[u8] = b"ChecksumI";
pub const PAD_BYTE: u8 = 0x00;
pub const MAX_IMAGE: usize = 4 * 1024 * 1024;
pub const DEFAULT_ADDRESS: u32 = 0;
/// "Erase      " + address + last page index.
pub const ERASE_COMMAND_SIZE: usize = ERASE_VERB.len() + 8;

pub const ERASE_TIMEOUT: Duration = Duration::from_secs(60);
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(10);
pub const CHECKSUM_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest single read, so Stop is noticed quickly.
pub const POLL: Duration = Duration::from_millis(250);
const IDLE: Duration = Duration::from_millis(50);

pub const RETRY: &str = "對講機目前可能沒有可用的韌體：請讓它保持在升級模式並重新執行升級；若仍失敗，\
將對講機關機後重新進入升級模式再試，或改用原廠 IAP 工具。";

/// A file that cannot be sent (Python: `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ImageError(pub String);

/// Upgrade failure. `erase_sent`: the erase command went out, so the radio's application may
/// already be gone (it stays in the bootloader, ready for a retry).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct IapError {
    pub message: String,
    pub erase_sent: bool,
}

impl From<IapError> for RadioError {
    fn from(error: IapError) -> Self {
        RadioError::Failed(error.message)
    }
}

/// A firmware file cut into the pages the bootloader receives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IapImage {
    padded: Vec<u8>,
    size: usize,
    address: u32,
}

impl IapImage {
    /// `data` for the default address, padded with 0x00 like the vendor tool.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ImageError> {
        Self::from_bytes_at(data, i64::from(DEFAULT_ADDRESS), PAD_BYTE)
    }

    /// `address` is checked like the Python version (0 ..= 0xFFFFFFFF).
    pub fn from_bytes_at(data: &[u8], address: i64, pad: u8) -> Result<Self, ImageError> {
        if data.is_empty() {
            return Err(ImageError("韌體檔是空的".into()));
        }
        if data.len() > MAX_IMAGE {
            return Err(ImageError(format!(
                "韌體檔太大（{} bytes）",
                group_thousands(data.len())
            )));
        }
        let address = u32::try_from(address).map_err(|_| ImageError("位址超出範圍".into()))?;
        let pages = data.len() / PAGE_SIZE + 1;
        let mut padded = data.to_vec();
        padded.resize(pages * PAGE_SIZE, pad);
        Ok(IapImage {
            padded,
            size: data.len(),
            address,
        })
    }

    /// Bytes in the file (without the padding).
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn address(&self) -> u32 {
        self.address
    }

    pub fn pages(&self) -> ChunksExact<'_, u8> {
        self.padded.chunks_exact(PAGE_SIZE)
    }

    pub fn page(&self, index: usize) -> Option<&[u8]> {
        self.padded.chunks_exact(PAGE_SIZE).nth(index)
    }

    pub fn page_count(&self) -> usize {
        self.padded.len() / PAGE_SIZE
    }

    pub fn last_page(&self) -> usize {
        self.page_count() - 1
    }

    /// Byte sum of every page sent, padding included (what the bootloader reports).
    pub fn checksum(&self) -> u32 {
        byte_sum(&self.padded)
    }

    pub fn erase_command(&self) -> Vec<u8> {
        // at most MAX_IMAGE / PAGE_SIZE, so it always fits
        let last = u32::try_from(self.last_page()).unwrap_or(u32::MAX);
        [
            &ERASE_VERB[..],
            &self.address.to_be_bytes(),
            &last.to_be_bytes(),
        ]
        .concat()
    }

    /// Every byte the PC sends, in order (used to compare with the vendor tool).
    pub fn stream(&self) -> Vec<u8> {
        [self.erase_command(), self.padded.clone()].concat()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IapResult {
    pub checksum: u32,
    pub pages: usize,
    /// The bootloader's answer to the erase command (non-ASCII bytes as U+FFFD).
    pub erase_reply: String,
}

/// Why an upgrade did not finish, before it is turned into the user's message.
enum Interrupt {
    /// The user pressed Stop.
    Stopped,
    /// The serial line failed (Python: `OSError`).
    Link(String),
    Failed(IapError),
}

fn link(error: RadioError) -> Interrupt {
    Interrupt::Link(error.to_string())
}

pub struct IapClient<'a, T: Transport + ?Sized> {
    transport: &'a mut T,
    progress: Progress<'a>,
    cancelled: Cancelled<'a>,
    pending: Vec<u8>,
    erase_sent: bool,
    pub erase_timeout: Duration,
    pub page_timeout: Duration,
    pub checksum_timeout: Duration,
    pub poll: Duration,
}

impl<'a, T: Transport + ?Sized> IapClient<'a, T> {
    pub fn new(transport: &'a mut T) -> Self {
        IapClient {
            transport,
            progress: Box::new(|_, _, _| {}),
            cancelled: Box::new(|| false),
            pending: Vec::new(),
            erase_sent: false,
            erase_timeout: ERASE_TIMEOUT,
            page_timeout: PAGE_TIMEOUT,
            checksum_timeout: CHECKSUM_TIMEOUT,
            poll: POLL,
        }
    }

    /// `progress(done, total, text)` after every step.
    pub fn with_progress(self, progress: impl FnMut(usize, usize, &str) + 'a) -> Self {
        IapClient {
            progress: Box::new(progress),
            ..self
        }
    }

    /// `cancelled()` is asked before every step and while waiting for a reply.
    pub fn with_cancel(self, cancelled: impl FnMut() -> bool + 'a) -> Self {
        IapClient {
            cancelled: Box::new(cancelled),
            ..self
        }
    }

    /// Whether the last upgrade sent the erase command.
    pub fn erase_sent(&self) -> bool {
        self.erase_sent
    }

    pub fn upgrade(&mut self, image: &IapImage) -> Result<IapResult, IapError> {
        self.erase_sent = false;
        let outcome = self.run(image);
        let erase_sent = self.erase_sent;
        outcome.map_err(|interrupt| match interrupt {
            Interrupt::Stopped => IapError {
                message: format!(
                    "已停止。{}",
                    if erase_sent {
                        RETRY
                    } else {
                        "未送出任何資料，對講機的韌體未變動。"
                    }
                ),
                erase_sent,
            },
            Interrupt::Link(detail) => IapError {
                message: format!(
                    "與對講機的連線中斷（{detail}）。{}",
                    if erase_sent {
                        RETRY
                    } else {
                        "未送出清除指令。"
                    }
                ),
                erase_sent,
            },
            Interrupt::Failed(error) => error,
        })
    }

    fn check_cancel(&mut self) -> Result<(), Interrupt> {
        if (self.cancelled)() {
            Err(Interrupt::Stopped)
        } else {
            Ok(())
        }
    }

    fn run(&mut self, image: &IapImage) -> Result<IapResult, Interrupt> {
        let total = image.page_count();
        self.transport.reset_input().map_err(link)?;
        self.pending.clear();
        self.check_cancel()?;
        (self.progress)(0, total, "傳送清除指令…");
        self.erase_sent = true;
        self.transport.write(&image.erase_command()).map_err(link)?;
        let text = format!(
            "等待對講機清除（最多 {:.0} 秒）…",
            self.erase_timeout.as_secs_f64()
        );
        (self.progress)(0, total, &text);
        let reply = self.wait_for(ERASE_ACK, self.erase_timeout, 0, |e| {
            format!(
                "清除指令已送出，但沒有收到對講機的確認，無法確定它是否已開始清除。\
                 請確認對講機已進入升級（IAP）模式、寫頻線已插緊、連接埠正確；\
                 若對講機仍停在升級模式，可重新執行升級。\n（{e}）"
            )
        })?;
        let erase_reply = ascii_text(&reply);
        self.send_pages(image)?;
        (self.progress)(total, total, "等待對講機校驗…");
        let wait = self.page_timeout + self.checksum_timeout;
        let tail = self.wait_for(CHECKSUM_TAG, wait, 4, |e| {
            format!("所有頁面已送出，但對講機沒有回傳校驗碼。{RETRY}\n（{e}）")
        })?;
        let got = u32::from_be_bytes(last_four(&tail));
        if got != image.checksum() {
            return Err(Interrupt::Failed(IapError {
                message: format!(
                    "校驗和不符（對講機 {got:#010x}，應為 {:#010x}），資料可能在傳輸中損壞，\
                     請勿直接開機使用。{RETRY}",
                    image.checksum()
                ),
                erase_sent: true,
            }));
        }
        (self.progress)(total, total, "升級完成");
        Ok(IapResult {
            checksum: got,
            pages: total,
            erase_reply,
        })
    }

    fn send_pages(&mut self, image: &IapImage) -> Result<(), Interrupt> {
        let total = image.page_count();
        for (index, page) in image.pages().enumerate() {
            self.check_cancel()?;
            (self.progress)(index, total, &format!("寫入第 {}/{total} 頁", index + 1));
            self.transport.write(page).map_err(link)?;
            if index == image.last_page() {
                break; // its "kyd" (if any) is consumed while waiting for the checksum
            }
            self.wait_for(PAGE_ACK, self.page_timeout, 0, |e| {
                format!("第 {}/{total} 頁沒有收到確認。{RETRY}\n（{e}）", index + 1)
            })?;
        }
        Ok(())
    }

    /// Consume replies up to and including `token` (case-insensitive, like the vendor tool,
    /// which only looks for substrings) plus `extra` bytes after it; anything that arrived
    /// later stays buffered for the next step. A timeout becomes `on_timeout(detail)`.
    fn wait_for(
        &mut self,
        token: &[u8],
        timeout: Duration,
        extra: usize,
        on_timeout: impl Fn(&str) -> String,
    ) -> Result<Vec<u8>, Interrupt> {
        let deadline = Instant::now() + timeout;
        let wanted = token.to_ascii_lowercase();
        loop {
            if let Some(at) = find(&self.pending.to_ascii_lowercase(), &wanted) {
                let end = at + token.len() + extra;
                if self.pending.len() >= end {
                    return Ok(self.pending.drain(..end).collect());
                }
            }
            self.check_cancel()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let seen = &self.pending[self.pending.len().saturating_sub(40)..];
                let detail = format!(
                    "等待 {} 逾時（收到 {}）",
                    String::from_utf8_lossy(token),
                    bytes_repr(seen)
                );
                return Err(self.timed_out(on_timeout(&detail)));
            }
            match self.transport.read_idle(IDLE, remaining.min(self.poll)) {
                Ok(data) => self.pending.extend(data),
                Err(RadioError::Timeout { message, .. }) => {
                    return Err(self.timed_out(on_timeout(&message)));
                }
                Err(RadioError::Failed(message)) => return Err(Interrupt::Link(message)),
            }
        }
    }

    fn timed_out(&self, message: String) -> Interrupt {
        Interrupt::Failed(IapError {
            message,
            erase_sent: self.erase_sent,
        })
    }
}

/// First occurrence of `needle` in `haystack`.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `bytes.decode("ascii", errors="replace")`.
pub(crate) fn ascii_text(data: &[u8]) -> String {
    data.iter()
        .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
        .collect()
}

/// The last four bytes as an array (zero-filled in front when shorter).
pub(crate) fn last_four(data: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    let tail = &data[data.len().saturating_sub(4)..];
    out[4 - tail.len()..].copy_from_slice(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(find(b"IAP Erase ok", b"ok"), Some(10));
        assert_eq!(find(b"o", b"ok"), None);
        assert_eq!(ascii_text(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(last_four(b"ChecksumI\x01\x02\x03\x04"), [1, 2, 3, 4]);
        assert_eq!(last_four(b"\x01"), [0, 0, 0, 1]);
    }
}
