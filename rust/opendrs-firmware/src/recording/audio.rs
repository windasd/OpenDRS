//! AMBE+2 audio: the decoder input, running the vendor's `ambep2.exe`, WAV files, and keeping
//! downloaded recordings on disk (port of the audio half of `recording.py` and of the non-GUI
//! part of `gui/recording_worker.py`).

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{FRAME_BYTES, RecordInfo, RecordingError, SAMPLE_RATE, failed};
use opendrs_core::pystr::{splitlines, str_repr, strip};

pub const DECODE_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the vendor installs the decoder (Windows only; `None` elsewhere).
pub fn default_ambep2() -> Option<PathBuf> {
    cfg!(windows).then(|| PathBuf::from(r"C:\wav\ambep2.exe"))
}

/// ambep2's input: one byte (0/1) per bit, MSB first, 72 per frame. Whole frames only.
pub fn to_bit_stream(data: &[u8]) -> Vec<u8> {
    let usable = data.len() - data.len() % FRAME_BYTES;
    data[..usable]
        .iter()
        .flat_map(|&b| (0..8).map(move |k| (b >> (7 - k)) & 1))
        .collect()
}

/// A private scratch directory, removed (ignoring errors, like Python's
/// `TemporaryDirectory(ignore_cleanup_errors=True)`) when dropped.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(prefix: &str) -> io::Result<Self> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir();
        let mut last_error = None;
        for _ in 0..100 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!(
                "{prefix}{}_{serial}_{nanos:08x}",
                std::process::id()
            ));
            match create_private_dir(&path) {
                Ok(()) => return Ok(ScratchDir(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_error = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("no free temporary directory name")))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // best effort, as in the Python version: a leftover temp folder is harmless
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// The exit code as Python reports it (unsigned on Windows, -signal on Unix).
fn exit_code(status: ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -i64::from(signal);
        }
    }
    #[cfg(windows)]
    {
        if let Some(code) = status.code() {
            return i64::from(u32::from_ne_bytes(code.to_ne_bytes()));
        }
    }
    status.code().map_or(-1, i64::from)
}

const DECODER_ARGS: [&str; 3] = ["-dec", "rec.bit", "rec.dat"];

fn run_failed(detail: impl std::fmt::Display) -> RecordingError {
    failed(format!("ambep2.exe 執行失敗：{detail}"))
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs the decoder in `dir` (bare file names: ambep2 is an ANSI program and cannot open paths
/// with characters outside the system code page, e.g. in %TEMP%).
fn run_decoder(ambep2: &Path, dir: &Path, timeout: Duration) -> Result<(), RecordingError> {
    // resolved against our working directory, like the is_file() check before
    let program = std::path::absolute(ambep2).map_err(run_failed)?;
    let mut command = Command::new(program);
    command
        .args(DECODER_ARGS)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    let mut child = command.spawn().map_err(run_failed)?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr {
            // whatever was read before a failure is still useful
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let status = match wait_with_timeout(&mut child, timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            // the child is being abandoned; failures to kill or reap it change nothing here
            let _ = child.kill();
            let _ = child.wait();
            let argv: Vec<String> = std::iter::once(ambep2.to_string_lossy().into_owned())
                .chain(DECODER_ARGS.iter().map(|a| (*a).to_owned()))
                .map(|a| str_repr(&a))
                .collect();
            return Err(run_failed(format!(
                "Command '[{}]' timed out after {:?} seconds",
                argv.join(", "),
                timeout.as_secs_f64()
            )));
        }
        Err(e) => {
            // cannot tell whether it is still running: stop it (best effort)
            let _ = child.kill();
            return Err(run_failed(e));
        }
    };
    let stderr = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    // Python decodes it as cp950; WHATWG Big5 is the same for the decoder's (ASCII) messages
    let (text, _) = encoding_rs::BIG5.decode_without_bom_handling(&stderr);
    let detail = splitlines(strip(&text)).last().copied().unwrap_or("");
    Err(failed(format!(
        "ambep2.exe 執行失敗（結束碼 {}）{detail}",
        exit_code(status)
    )))
}

/// 16-bit 8 kHz mono PCM, decoded by the vendor's ambep2.exe.
pub fn decode_ambe(
    data: &[u8],
    ambep2: &Path,
    timeout: Duration,
) -> Result<Vec<u8>, RecordingError> {
    if !ambep2.is_file() {
        return Err(failed(format!(
            "找不到 ambep2.exe（{}），無法把錄音轉成聲音",
            ambep2.display()
        )));
    }
    let io_failed = |e: io::Error| failed(e.to_string());
    let scratch = ScratchDir::new("opendrs_rec_").map_err(io_failed)?;
    fs::write(scratch.0.join("rec.bit"), to_bit_stream(data)).map_err(io_failed)?;
    run_decoder(ambep2, &scratch.0, timeout)?;
    let pcm = scratch.0.join("rec.dat");
    if !pcm.is_file() {
        return Err(failed("ambep2.exe 沒有產生輸出"));
    }
    fs::read(pcm).map_err(io_failed)
}

/// The bytes Python's `wave` module writes for 16-bit mono PCM at 8 kHz.
pub fn wav_bytes(pcm: &[u8]) -> Vec<u8> {
    let frames = &pcm[..pcm.len() - pcm.len() % 2];
    // a WAV cannot describe more than u32::MAX bytes; Python's wave refuses such data too
    let size = u32::try_from(frames.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + frames.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&size.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(frames);
    out
}

pub fn write_wav(path: &Path, pcm: &[u8]) -> io::Result<PathBuf> {
    fs::write(path, wav_bytes(pcm))?;
    Ok(path.to_path_buf())
}

// ---------------------------------------------------------------------- saving

/// What happened to one downloaded recording (the non-GUI part of `gui/recording_worker.py`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub info: RecordInfo,
    pub raw: PathBuf,
    /// `None` when there was no decoder or decoding failed (see `note`).
    pub wav: Option<PathBuf>,
    pub note: String,
}

pub const NO_DECODER_NOTE: &str = "未設定 ambep2.exe（AMBE+2 解碼器），只保存原始資料";
pub const MISSING_DECODER_NOTE: &str = "找不到 ambep2.exe，只保存原始資料";

/// `info.stem()`, or with a suffix when a different recording already has that name.
pub fn free_stem(out_dir: &Path, info: &RecordInfo, data: &[u8]) -> io::Result<String> {
    let base = info.stem();
    let mut stem = base.clone();
    let mut n = 1;
    loop {
        let path = out_dir.join(format!("{stem}.ambe"));
        if !path.exists() || fs::read(&path)? == data {
            return Ok(stem);
        }
        n += 1;
        stem = format!("{base}_{n}");
    }
}

/// Keep the raw AMBE+2 data in `out_dir`, then convert it to WAV when a decoder is available.
/// Only a failure to store the raw data is an error; a failed conversion is a note.
pub fn save_recording(
    out_dir: &Path,
    info: &RecordInfo,
    data: &[u8],
    ambep2: Option<&Path>,
) -> Result<Saved, RecordingError> {
    let store = || -> io::Result<(String, PathBuf)> {
        fs::create_dir_all(out_dir)?;
        let stem = free_stem(out_dir, info, data)?;
        let raw = out_dir.join(format!("{stem}.ambe"));
        fs::write(&raw, data)?;
        Ok((stem, raw))
    };
    let (stem, raw) = store().map_err(|e| {
        failed(format!(
            "第 {} 筆無法儲存到 {}：{e}",
            info.number,
            out_dir.display()
        ))
    })?;
    let saved = |wav: Option<PathBuf>, note: &str| Saved {
        info: info.clone(),
        raw: raw.clone(),
        wav,
        note: note.to_owned(),
    };
    let Some(ambep2) = ambep2 else {
        return Ok(saved(None, NO_DECODER_NOTE));
    };
    if !ambep2.is_file() {
        return Ok(saved(None, MISSING_DECODER_NOTE));
    }
    let wav = decode_ambe(data, ambep2, DECODE_TIMEOUT).and_then(|pcm| {
        write_wav(&out_dir.join(format!("{stem}.wav")), &pcm).map_err(|e| failed(e.to_string()))
    });
    Ok(match wav {
        Ok(path) => saved(Some(path), ""),
        Err(e) => saved(None, &e.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_dir_is_created_and_removed() {
        let dir = ScratchDir::new("opendrs_test_").expect("scratch");
        let path = dir.0.clone();
        assert!(path.is_dir());
        drop(dir);
        assert!(!path.exists());
    }
}
