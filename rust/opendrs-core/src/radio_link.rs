//! Read / write a radio over the programming cable (port of `radio_link.py`).
//!
//! ```text
//! PC -> "Flash Read " | "Flash Write"  11-byte verb + start address u32 BE + block count u32 BE
//!       + write password 6 bytes + read password 6 bytes                       (31 bytes)
//! RF -> "  Read_2M_\0\0" + zones, channels, contacts, part1, part2 (u16 LE) + identity
//!       "  Write_2M_\0" + identity
//! read:  PC "Read" -> RF 2048 bytes per block; then RF "ChecksumR" + u32 BE byte sum
//! write: PC 2048 bytes -> RF "Write" (not after the last block); then RF "ChecksumW" + u32 BE
//!        sum, followed by free-form debug text.
//! ```
//!
//! The radio stages a write and only commits it once every block has arrived; the checksum
//! arrives after the commit, so callers keep a backup read taken just before writing.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::codeplug::{BLOCK, CodeplugError, Counts, PART1_ADDR, build_stream, parse_stream};
use crate::f136::{self, Project};

pub const BAUD: u32 = 115_200;
pub const READ_VERB: &[u8; 11] = b"Flash Read ";
pub const WRITE_VERB: &[u8; 11] = b"Flash Write";
pub const READ_PREFIX: &[u8] = b"  Read_2M_\0\0";
pub const WRITE_PREFIX: &[u8] = b"  Write_2M_\0";
pub const READ_REQUEST: &[u8] = b"Read";
pub const WRITE_ACK: &[u8] = b"Write";
pub const CHECKSUM_READ: &[u8] = b"ChecksumR";
pub const CHECKSUM_WRITE: &[u8] = b"ChecksumW";
/// The legacy CPS always sent 6 here for reads; the radio recomputes the count itself.
pub const LEGACY_READ_BLOCKS: u32 = 6;
pub const PASSWORD_SIZE: usize = 6;
pub const COMMAND_SIZE: usize = 31;

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const BLOCK_TIMEOUT: Duration = Duration::from_secs(6);
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
const CHECKSUM_TIMEOUT: Duration = Duration::from_secs(15);

pub const NO_REPLY: &str = "對講機沒有任何回應。請確認：\n\
1. 對講機已開機，停在待機畫面（不在選單、掃描或通話中）；\n\
2. 寫頻線兩端都插緊（尤其對講機側的插頭）；\n\
3. 連接埠選的是寫頻線（目前的 COM 埠）；\n\
4. 仍不行時將對講機關機再開機後重試。";

/// Communication failure, with a message suitable for the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RadioError {
    /// Nothing (or too little) arrived in time; `received` counts the bytes that did.
    #[error("{message}")]
    Timeout { message: String, received: usize },
    #[error("{0}")]
    Failed(String),
}

fn failed(message: impl Into<String>) -> RadioError {
    RadioError::Failed(message.into())
}

/// What the protocol needs from a serial line (or from the simulated radio).
pub trait Transport {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError>;
    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError>;
    /// Read until nothing new has arrived for `idle`, or `max_wait` has passed.
    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError>;
    fn reset_input(&mut self) -> Result<(), RadioError>;
}

/// Python's `repr(bytes)`, so messages read exactly like the Python version's.
pub fn bytes_repr(data: &[u8]) -> String {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &b in data {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if b as char == quote => out.push_str(&format!("\\{quote}")),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote);
    out
}

// ------------------------------------------------------------------ serial port

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub device: String,
    pub description: String,
    pub is_programming_cable: bool,
}

/// Prolific, WCH, Silicon Labs, FTDI.
pub const KNOWN_CABLE_VIDS: [u16; 4] = [0x067B, 0x1A86, 0x10C4, 0x0403];

/// Serial ports, likely programming cables first.
pub fn list_ports() -> Result<Vec<PortInfo>, RadioError> {
    let ports =
        serialport::available_ports().map_err(|e| failed(format!("無法列出連接埠：{e}")))?;
    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| {
            let (description, vid) = match &p.port_type {
                serialport::SerialPortType::UsbPort(usb) => (
                    usb.product.clone().unwrap_or_else(|| p.port_name.clone()),
                    usb.vid,
                ),
                _ => (p.port_name.clone(), 0),
            };
            PortInfo {
                device: p.port_name,
                description,
                is_programming_cable: KNOWN_CABLE_VIDS.contains(&vid),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (!a.is_programming_cable, &a.device).cmp(&(!b.is_programming_cable, &b.device))
    });
    Ok(out)
}

const POLL: Duration = Duration::from_millis(50);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    pub fn open(name: &str) -> Result<Self, RadioError> {
        let mut port = serialport::new(name, BAUD)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .timeout(POLL)
            .open()
            .map_err(|e| failed(format!("無法開啟 {name}：{e}")))?;
        port.write_data_terminal_ready(true)
            .map_err(|e| failed(format!("無法設定 {name}：{e}")))?;
        port.write_request_to_send(true)
            .map_err(|e| failed(format!("無法設定 {name}：{e}")))?;
        Ok(SerialTransport { port })
    }

    fn read_some(&mut self, buf: &mut [u8]) -> Result<usize, RadioError> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(failed(format!("讀取寫頻線失敗：{e}"))),
        }
    }
}

impl Transport for SerialTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        let io_error = |e: io::Error| failed(format!("寫入寫頻線失敗：{e}"));
        self.port
            .set_timeout(WRITE_TIMEOUT)
            .map_err(|e| failed(e.to_string()))?;
        let result = self.port.write_all(data).and_then(|()| self.port.flush());
        self.port
            .set_timeout(POLL)
            .map_err(|e| failed(e.to_string()))?;
        result.map_err(io_error)
    }

    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        let deadline = Instant::now() + timeout;
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n {
            if Instant::now() > deadline {
                return Err(RadioError::Timeout {
                    message: format!("等待對講機回應逾時（預期 {n} bytes，收到 {got}）"),
                    received: got,
                });
            }
            got += self.read_some(&mut buf[got..])?;
        }
        Ok(buf)
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        let mut out = Vec::new();
        let start = Instant::now();
        let mut last = start;
        let mut chunk = [0u8; 4096];
        loop {
            let n = self.read_some(&mut chunk)?;
            let now = Instant::now();
            if n > 0 {
                out.extend_from_slice(&chunk[..n]);
                last = now;
            } else if !out.is_empty() && now - last >= idle {
                break;
            }
            if now - start >= max_wait {
                break;
            }
        }
        Ok(out)
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.port
            .clear(serialport::ClearBuffer::Input)
            .map_err(|e| failed(format!("無法清除緩衝區：{e}")))
    }
}

// ------------------------------------------------------------------ protocol

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioIdentity {
    pub firmware: String,
    pub model: String,
    pub band: String,
    pub raw: Vec<u8>,
}

static FIRMWARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{3}[0-9A-F]\.").expect("valid regex"));
static MODEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+([A-Z]{1,4}-[0-9A-Z]+)").expect("valid regex"));
static BAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+(\d+-\d+)MHZ\+").expect("valid regex"));

impl RadioIdentity {
    pub fn parse(raw: &[u8]) -> Self {
        let text: String = raw
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '\0'
                }
            })
            .collect();
        let parts: Vec<&str> = text.split('\0').filter(|p| !p.is_empty()).collect();
        let first_group = |re: &Regex| {
            parts
                .iter()
                .find_map(|p| re.captures(p).map(|c| c[1].to_string()))
                .unwrap_or_default()
        };
        RadioIdentity {
            firmware: parts
                .iter()
                .find(|p| FIRMWARE.is_match(p))
                .map(|p| p.to_string())
                .unwrap_or_default(),
            model: first_group(&MODEL),
            band: first_group(&BAND),
            raw: raw.to_vec(),
        }
    }

    pub fn summary(&self) -> String {
        let model = if self.model.is_empty() {
            "未知機型"
        } else {
            &self.model
        };
        let band = if self.band.is_empty() {
            String::new()
        } else {
            format!("{} MHz", self.band)
        };
        let firmware = if self.firmware.is_empty() {
            "?"
        } else {
            &self.firmware
        };
        format!("{model}　{band}　韌體 {firmware}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadResult {
    pub project: Project,
    pub stream: Vec<u8>,
    pub identity: RadioIdentity,
    pub counts: Counts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub checksum: u32,
    pub radio_log: String,
}

/// Why `project` must not be written to the radio that reported `identity`.
pub fn identity_mismatch(project: &Project, identity: &RadioIdentity) -> Option<String> {
    if identity.model.is_empty() || identity.band.is_empty() {
        return Some("無法辨識對講機的機型或頻段".into());
    }
    let dev = project.device();
    if !dev.model.is_empty() && dev.model != identity.model {
        return Some(format!(
            "專案機型為 {}，對講機是 {}",
            dev.model, identity.model
        ));
    }
    let band = dev.band_label.trim_start_matches('F');
    if !band.is_empty() && band != identity.band {
        return Some(format!(
            "專案頻段為 {band} MHz，對講機是 {} MHz",
            identity.band
        ));
    }
    None
}

/// What to write back so only the radio's clock changes: the project just read, provided it
/// rebuilds the transfer that was read byte for byte.
pub fn clock_write_project(read: &ReadResult) -> Result<Project, RadioError> {
    let rebuilt = build_stream(&read.project, None)
        .map_err(|e| failed(format!("已中止，未寫入任何資料：讀回的資料無法寫回（{e}）")))?;
    if rebuilt != read.stream {
        return Err(failed(
            "已中止，未寫入任何資料：讀回的資料無法逐位元組重建，為了安全不寫回。",
        ));
    }
    Ok(read.project.clone())
}

/// Keep what was read from the radio: the project (.F136) and the raw transfer (.bin).
pub fn save_backup(
    result: &ReadResult,
    directory: &Path,
    kind: &str,
    stamp: &str,
) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let model: String = if result.identity.model.is_empty() {
        "radio".into()
    } else {
        result.identity.model.clone()
    };
    let model: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = directory.join(format!("{stamp}_{kind}_{model}"));
    fs::write(base.with_extension("bin"), &result.stream)?;
    let path = base.with_extension("F136");
    fs::write(&path, f136::serialize(&result.project))?;
    Ok(path)
}

/// The 31-byte command that starts a transfer.
pub fn command(
    verb: &[u8; 11],
    blocks: u32,
    write_password: &[u8],
    read_password: &[u8],
) -> Result<Vec<u8>, RadioError> {
    if write_password.len() > PASSWORD_SIZE || read_password.len() > PASSWORD_SIZE {
        return Err(failed("密碼最多 6 bytes"));
    }
    let mut out = Vec::with_capacity(COMMAND_SIZE);
    out.extend_from_slice(verb);
    out.extend_from_slice(&PART1_ADDR.to_be_bytes());
    out.extend_from_slice(&blocks.to_be_bytes());
    for pw in [write_password, read_password] {
        out.extend_from_slice(pw);
        out.resize(out.len() + PASSWORD_SIZE - pw.len(), 0);
    }
    Ok(out)
}

fn checksum(tail: &[u8], tag: &[u8]) -> Result<u32, RadioError> {
    if !tail.starts_with(tag) || tail.len() < tag.len() + 4 {
        return Err(failed(format!(
            "對講機未回傳校驗碼（收到 {}）",
            bytes_repr(&tail[..tail.len().min(16)])
        )));
    }
    let v = &tail[tag.len()..tag.len() + 4];
    Ok(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
}

pub fn byte_sum(data: &[u8]) -> u32 {
    data.iter()
        .fold(0u32, |acc, &b| acc.wrapping_add(u32::from(b)))
}

pub type Progress<'a> = Box<dyn FnMut(usize, usize, &str) + 'a>;
/// Vetoes a write after seeing the radio's identity: `Some(reason)` aborts it.
pub type IdentityCheck<'a> = &'a dyn Fn(&RadioIdentity) -> Option<String>;

pub struct RadioClient<'a, T: Transport + ?Sized> {
    transport: &'a mut T,
    progress: Progress<'a>,
}

impl<'a, T: Transport + ?Sized> RadioClient<'a, T> {
    pub fn new(transport: &'a mut T) -> Self {
        RadioClient {
            transport,
            progress: Box::new(|_, _, _| {}),
        }
    }

    pub fn with_progress(
        transport: &'a mut T,
        progress: impl FnMut(usize, usize, &str) + 'a,
    ) -> Self {
        RadioClient {
            transport,
            progress: Box::new(progress),
        }
    }

    /// The radio's answer to a command; silence gets a checklist instead of a byte count.
    fn first_reply(&mut self, n: usize) -> Result<Vec<u8>, RadioError> {
        match self.transport.read_exact(n, REPLY_TIMEOUT) {
            Err(RadioError::Timeout { received: 0, .. }) => Err(RadioError::Timeout {
                message: NO_REPLY.into(),
                received: 0,
            }),
            other => other,
        }
    }

    pub fn read_codeplug(&mut self) -> Result<ReadResult, RadioError> {
        self.transport.reset_input()?;
        (self.progress)(0, 1, "連線中…");
        self.transport
            .write(&command(READ_VERB, LEGACY_READ_BLOCKS, b"", b"")?)?;
        let head = self.first_reply(READ_PREFIX.len() + 10)?;
        if !head.starts_with(READ_PREFIX) {
            return Err(failed(format!(
                "對講機回應不符（{}）。請確認機型、連線與寫頻線。",
                bytes_repr(&head[..head.len().min(12)])
            )));
        }
        let field = |i: usize| {
            let at = READ_PREFIX.len() + 2 * i;
            usize::from(u16::from_le_bytes([head[at], head[at + 1]]))
        };
        let counts = Counts::new(field(0), field(1), field(2));
        let (p1, p2) = (field(3), field(4));
        if (counts.part1_blocks(), counts.part2_blocks()) != (p1, p2) {
            return Err(failed(format!(
                "對講機回報的區塊數 ({p1}+{p2}) 與計數不符，停止讀取"
            )));
        }
        let identity = RadioIdentity::parse(
            &self
                .transport
                .read_idle(Duration::from_millis(300), Duration::from_secs(3))?,
        );
        let total = counts.total_blocks();
        let mut stream = Vec::with_capacity(total * BLOCK);
        for i in 0..total {
            (self.progress)(i, total, &format!("讀取區塊 {}/{total}", i + 1));
            self.transport.write(READ_REQUEST)?;
            stream.extend(self.transport.read_exact(BLOCK, BLOCK_TIMEOUT)?);
        }
        let tail = self
            .transport
            .read_exact(CHECKSUM_READ.len() + 4, CHECKSUM_TIMEOUT)?;
        if byte_sum(&stream) != checksum(&tail, CHECKSUM_READ)? {
            return Err(failed("讀取資料的校驗和不符，資料可能在傳輸中損壞，請重試"));
        }
        (self.progress)(total, total, "讀取完成");
        let project = parse_stream(&stream, counts)
            .map_err(|e: CodeplugError| failed(format!("讀到的資料格式不符：{e}")))?;
        Ok(ReadResult {
            project,
            stream,
            identity,
            counts,
        })
    }

    /// Write `project`. `expect` may veto the write after seeing the radio's identity; nothing has
    /// been sent at that point, so the radio simply times out and keeps its data.
    pub fn write_codeplug(
        &mut self,
        project: &Project,
        date_text: Option<&str>,
        expect: Option<IdentityCheck<'_>>,
    ) -> Result<WriteResult, RadioError> {
        let stream =
            build_stream(project, date_text).map_err(|e| failed(format!("專案無法寫入：{e}")))?;
        let total = Counts::of(project).total_blocks();
        self.transport.reset_input()?;
        (self.progress)(0, total, "連線中…");
        let blocks = u32::try_from(total).map_err(|_| failed("區塊數過多"))?;
        self.transport
            .write(&command(WRITE_VERB, blocks, b"", b"")?)?;
        let head = self.first_reply(WRITE_PREFIX.len())?;
        if head != WRITE_PREFIX {
            return Err(failed(format!(
                "對講機回應不符（{}）。請確認機型、連線與寫頻線。",
                bytes_repr(&head)
            )));
        }
        let identity = RadioIdentity::parse(
            &self
                .transport
                .read_idle(Duration::from_millis(300), Duration::from_secs(3))?,
        );
        if let Some(reason) = expect.and_then(|check| check(&identity)) {
            return Err(failed(format!("已中止，未寫入任何資料：{reason}")));
        }
        for (i, block) in stream.chunks(BLOCK).enumerate() {
            (self.progress)(i, total, &format!("寫入區塊 {}/{total}", i + 1));
            self.transport.write(block)?;
            if i + 1 < total {
                let ack = match self.transport.read_exact(WRITE_ACK.len(), ACK_TIMEOUT) {
                    Err(RadioError::Timeout { .. }) => b"(no reply)".to_vec(),
                    other => other?,
                };
                if ack != WRITE_ACK {
                    return Err(failed(format!(
                        "第 {} 塊未獲確認（{}）。對講機不會套用不完整的資料，原本的設定仍保留。",
                        i + 1,
                        bytes_repr(&ack)
                    )));
                }
            }
        }
        (self.progress)(total, total, "等待對講機確認…");
        let tail = self
            .transport
            .read_exact(CHECKSUM_WRITE.len() + 4, CHECKSUM_TIMEOUT)?;
        let got = checksum(&tail, CHECKSUM_WRITE)?;
        let log_bytes = self
            .transport
            .read_idle(Duration::from_secs(1), Duration::from_secs(20))?;
        let radio_log: String = log_bytes
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
            .collect();
        if got != byte_sum(&stream) {
            return Err(failed(
                "對講機收到的資料校驗和不符，已套用的設定可能損壞。請重新寫入，或寫回寫入前的備份。",
            ));
        }
        (self.progress)(total, total, "寫入完成");
        Ok(WriteResult {
            checksum: got,
            radio_log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python() {
        assert_eq!(bytes_repr(b"  Read_2M_\0\0"), "b'  Read_2M_\\x00\\x00'");
        assert_eq!(bytes_repr(b"(no reply)"), "b'(no reply)'");
        assert_eq!(bytes_repr(b"it's"), "b\"it's\"");
        assert_eq!(bytes_repr(b"a\r\n"), "b'a\\r\\n'");
    }

    #[test]
    fn command_layout() {
        let cmd = command(READ_VERB, 6, b"", b"").unwrap();
        assert_eq!(cmd.len(), COMMAND_SIZE);
        assert_eq!(&cmd[11..19], &[0, 0x3C, 0, 0, 0, 0, 0, 6]);
        assert!(command(READ_VERB, 6, b"1234567", b"").is_err());
    }
}
