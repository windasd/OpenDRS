//! The read / write / clock-sync sequences, shared by the CLI and the GUI (port of `RadioWorker`
//! in `gui/radio_panel.py` and the dialog texts in `gui/radio_ops.py`).
//!
//! A [`RadioSession`] runs one blocking operation at a time on a transport the caller has opened.
//! A GUI builds it inside a background thread and forwards the progress and status callbacks to
//! the UI thread (over a channel, for instance). The safety order of a write is that of the
//! Python worker: read the radio and back it up as `before_write` → refuse on a model / band
//! mismatch → pause → write, dated now, with the identity checked again during the handshake.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};

use crate::backups::{BEFORE_WRITE, STAMP_FORMAT};
use crate::codeplug::{Counts, DATE_FORMAT};
use crate::f136::Project;
use crate::radio_link::{
    Progress, RadioClient, RadioError, RadioIdentity, ReadResult, Transport, WriteResult,
    clock_write_project, identity_mismatch, save_backup,
};

/// Let the radio return to its command loop after a read before the write starts.
pub const PAUSE_BETWEEN_COMMANDS: Duration = Duration::from_millis(1500);
const READ_KIND: &str = "read";

/// What a session does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Read,
    Write,
    Clock,
}

impl Mode {
    /// 讀取／寫入／校正時鐘, as in the status lines.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Read => "讀取",
            Mode::Write => "寫入",
            Mode::Clock => "校正時鐘",
        }
    }

    /// The title of the dialogs that start the operation and report its result.
    pub fn title(self) -> &'static str {
        match self {
            Mode::Read => "讀取對講機",
            Mode::Write => "寫入對講機",
            Mode::Clock => "校正對講機時鐘",
        }
    }
}

/// A status-line update from a session; `Display` gives the line the GUI shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status<'s> {
    /// `{mode}：連線到 {port}…`
    Connecting { mode: Mode, port: &'s str },
    /// `寫入前先讀取對講機目前的設定並備份…`
    BackingUp,
    /// A read has been backed up: `已讀取：{summary}`.
    Read {
        identity: &'s RadioIdentity,
        backup: &'s Path,
    },
    /// The radio's settings before a write have been backed up: `備份完成：{file}（{summary}）`.
    BackedUp {
        identity: &'s RadioIdentity,
        backup: &'s Path,
    },
    /// `開始寫入…`
    Writing,
    /// `{mode}完成`
    Done(Mode),
    /// `錯誤：{message}` (the text of [`SessionError::message`]).
    Failed(&'s str),
}

impl fmt::Display for Status<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Connecting { mode, port } => write!(f, "{}：連線到 {port}…", mode.name()),
            Status::BackingUp => f.write_str("寫入前先讀取對講機目前的設定並備份…"),
            Status::Read { identity, .. } => write!(f, "已讀取：{}", identity.summary()),
            Status::BackedUp { identity, backup } => write!(
                f,
                "備份完成：{}（{}）",
                backup.file_name().map_or_else(
                    || backup.display().to_string(),
                    |n| n.to_string_lossy().into()
                ),
                identity.summary()
            ),
            Status::Writing => f.write_str("開始寫入…"),
            Status::Done(mode) => write!(f, "{}完成", mode.name()),
            Status::Failed(message) => write!(f, "錯誤：{message}"),
        }
    }
}

/// Why a session stopped.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The radio did not answer as expected, or the operation was refused for safety.
    #[error(transparent)]
    Radio(#[from] RadioError),
    /// The backup could not be saved; nothing was written to the radio.
    #[error("無法保存備份")]
    Backup(#[source] io::Error),
}

impl SessionError {
    /// The text the GUI reports: the radio's error message, or 未預期的錯誤：… otherwise.
    pub fn message(&self) -> String {
        match self {
            SessionError::Radio(e) => e.to_string(),
            SessionError::Backup(e) => format!("未預期的錯誤：無法保存備份（{e}）"),
        }
    }
}

/// A finished read: what the radio holds and where its backup went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOutcome {
    pub read: ReadResult,
    pub backup: PathBuf,
}

/// A finished write (or clock sync).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOutcome {
    /// What the radio held just before the write, as saved in `backup`.
    pub before: ReadResult,
    pub backup: PathBuf,
    /// The radio's confirmation: the checksum it computed and its debug text.
    pub written: WriteResult,
    /// The date stamp written; the radio set its clock from it.
    pub date_text: String,
}

type StatusSink<'a> = Box<dyn FnMut(&Status<'_>) + 'a>;
type Now<'a> = Box<dyn Fn() -> NaiveDateTime + 'a>;

/// Runs reads and writes on an open transport, backing up every read.
pub struct RadioSession<'a> {
    transport: &'a mut dyn Transport,
    backup_dir: PathBuf,
    port_label: String,
    progress: Progress<'a>,
    status: StatusSink<'a>,
    pause: Duration,
    now: Now<'a>,
}

impl<'a> RadioSession<'a> {
    /// A session with no callbacks, the 1.5 s pause before writing and the PC's local time.
    pub fn new(transport: &'a mut dyn Transport, backup_dir: impl Into<PathBuf>) -> Self {
        RadioSession {
            transport,
            backup_dir: backup_dir.into(),
            port_label: String::new(),
            progress: Box::new(|_, _, _| {}),
            status: Box::new(|_| {}),
            pause: PAUSE_BETWEEN_COMMANDS,
            now: Box::new(|| Local::now().naive_local()),
        }
    }

    /// The port as the user picked it, for the `連線到 {port}…` line.
    pub fn with_port_label(self, port_label: impl Into<String>) -> Self {
        RadioSession {
            port_label: port_label.into(),
            ..self
        }
    }

    /// Block progress of every transfer: (done, total, text).
    pub fn with_progress(self, progress: impl FnMut(usize, usize, &str) + 'a) -> Self {
        RadioSession {
            progress: Box::new(progress),
            ..self
        }
    }

    /// Status-line updates.
    pub fn with_status(self, status: impl FnMut(&Status<'_>) + 'a) -> Self {
        RadioSession {
            status: Box::new(status),
            ..self
        }
    }

    /// The pause between the read and the write (zero for the simulated radio).
    pub fn with_pause(self, pause: Duration) -> Self {
        RadioSession { pause, ..self }
    }

    /// The clock used for backup names and the date written (tests fix it).
    pub fn with_now(self, now: impl Fn() -> NaiveDateTime + 'a) -> Self {
        RadioSession {
            now: Box::new(now),
            ..self
        }
    }

    /// Read the radio and back it up as `read`.
    pub fn read(&mut self) -> Result<ReadOutcome, SessionError> {
        self.reporting(Mode::Read, |session| {
            let read = session.client().read_codeplug()?;
            let backup = session.backup(&read, READ_KIND)?;
            session.notify(Status::Read {
                identity: &read.identity,
                backup: &backup,
            });
            Ok(ReadOutcome { read, backup })
        })
    }

    /// Write `project`: read and back up the radio first, refuse on a model / band mismatch.
    pub fn write(&mut self, project: &Project) -> Result<WriteOutcome, SessionError> {
        self.reporting(Mode::Write, |session| {
            let (before, backup) = session.read_before_write()?;
            session.write_checked(project, before, backup)
        })
    }

    /// Set the radio's clock: write back what was read, provided it rebuilds byte for byte, so
    /// only the date stamp changes.
    pub fn sync_clock(&mut self) -> Result<WriteOutcome, SessionError> {
        self.reporting(Mode::Clock, |session| {
            let (before, backup) = session.read_before_write()?;
            let target = clock_write_project(&before)?;
            session.write_checked(&target, before, backup)
        })
    }

    /// Announce `mode`, run it, then report 完成 or the error.
    fn reporting<T>(
        &mut self,
        mode: Mode,
        run: impl FnOnce(&mut Self) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        (self.status)(&Status::Connecting {
            mode,
            port: &self.port_label,
        });
        let result = run(self);
        match &result {
            Ok(_) => self.notify(Status::Done(mode)),
            Err(e) => self.notify(Status::Failed(&e.message())),
        }
        result
    }

    fn notify(&mut self, status: Status<'_>) {
        (self.status)(&status);
    }

    fn client(&mut self) -> RadioClient<'_, dyn Transport + 'a> {
        let progress = &mut self.progress;
        RadioClient::with_progress(&mut *self.transport, move |done, total, text: &str| {
            progress(done, total, text)
        })
    }

    fn backup(&self, read: &ReadResult, kind: &str) -> Result<PathBuf, SessionError> {
        let stamp = (self.now)().format(STAMP_FORMAT).to_string();
        save_backup(read, &self.backup_dir, kind, &stamp).map_err(SessionError::Backup)
    }

    fn read_before_write(&mut self) -> Result<(ReadResult, PathBuf), SessionError> {
        self.notify(Status::BackingUp);
        let before = self.client().read_codeplug()?;
        let backup = self.backup(&before, BEFORE_WRITE)?;
        self.notify(Status::BackedUp {
            identity: &before.identity,
            backup: &backup,
        });
        Ok((before, backup))
    }

    fn write_checked(
        &mut self,
        target: &Project,
        before: ReadResult,
        backup: PathBuf,
    ) -> Result<WriteOutcome, SessionError> {
        if let Some(reason) = identity_mismatch(target, &before.identity) {
            return Err(RadioError::Failed(format!("已中止，未寫入任何資料：{reason}")).into());
        }
        std::thread::sleep(self.pause);
        self.notify(Status::Writing);
        let date_text = (self.now)().format(DATE_FORMAT).to_string();
        let expect = |identity: &RadioIdentity| identity_mismatch(target, identity);
        let written = self
            .client()
            .write_codeplug(target, Some(&date_text), Some(&expect))?;
        Ok(WriteOutcome {
            before,
            backup,
            written,
            date_text,
        })
    }
}

// ------------------------------------------------------------------ dialog texts

fn or_unknown(text: &str) -> &str {
    if text.is_empty() { "?" } else { text }
}

/// Model, band and counts of the project about to be written.
pub fn write_summary(project: &Project) -> String {
    let c = Counts::of(project);
    let dev = project.device();
    format!(
        "機型 {}　{}\n分區 {}、頻道 {}、聯絡人 {}（{} 個區塊）",
        or_unknown(&dev.model),
        or_unknown(&dev.band_label),
        c.zones,
        c.channels,
        c.contacts,
        c.total_blocks()
    )
}

/// The question before a write (title [`Mode::title`]).
pub fn write_confirmation(port_label: &str, project: &Project, backup_dir: &Path) -> String {
    format!(
        "要寫入到 {port_label} 嗎？\n\n{}\n\n寫入前會先讀取對講機並備份到 {}；機型或頻段不符會中止。",
        write_summary(project),
        backup_dir.display()
    )
}

/// The question before a clock sync (title [`Mode::title`]).
pub fn clock_confirmation(port_label: &str) -> String {
    format!(
        "要把 {port_label} 上的對講機時鐘設為電腦目前的時間嗎？\n\n\
         做法：先讀取對講機目前的設定並備份，再把同樣的資料原樣寫回，只更新「寫入時間」；\
         對講機在寫入完成時依此設定時鐘（精確到分鐘，秒數不變），然後重新啟動。\n\n\
         讀回的資料若無法逐位元組重建就中止，不寫入任何資料。目前編輯中的專案不受影響。"
    )
}

fn clock_set_line(outcome: &WriteOutcome) -> String {
    format!(
        "對講機時鐘已設為 {}（開始寫入時的電腦時間，秒數維持對講機原值）。",
        outcome.date_text
    )
}

fn backed_up_line(outcome: &WriteOutcome) -> String {
    format!("寫入前的設定已備份到：{}", outcome.backup.display())
}

/// The status-bar message after a read.
pub fn read_success_text(outcome: &ReadOutcome) -> String {
    format!(
        "已讀取 {}，備份：{}",
        outcome.read.identity.summary(),
        outcome.backup.display()
    )
}

/// The message after a write.
pub fn write_success_text(outcome: &WriteOutcome) -> String {
    [
        format!(
            "寫入完成，對講機確認校驗和 0x{:08X}。",
            outcome.written.checksum
        ),
        clock_set_line(outcome),
        backed_up_line(outcome),
        "對講機正在套用設定，請等它重新啟動後再操作。".to_string(),
    ]
    .join("\n")
}

/// The message after a clock sync.
pub fn clock_success_text(outcome: &WriteOutcome) -> String {
    [
        clock_set_line(outcome),
        "對講機的設定原樣寫回，沒有其他變更；目前編輯中的專案不受影響。".to_string(),
        backed_up_line(outcome),
        "對講機正在重新啟動；之後可在對講機選單的「日期／時間」確認。".to_string(),
    ]
    .join("\n")
}
