//! 列印／預覽列印／匯出 PDF without a print engine of our own (port of `print_ops.py`): the
//! report is written as an HTML file and opened in the system browser, which previews and prints
//! it (A4 landscape); a PDF is made by a headless Edge or Chrome when one is installed.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::app::OpenDrsApp;
use crate::dialogs::open_with_system;
use crate::report::report_html;

const NO_BROWSER: &str = "找不到 Microsoft Edge 或 Google Chrome，無法產生 PDF；可改用「預覽列印」在瀏覽器中另存為 PDF。";

/// Printing rules the Qt version set on its printer: A4 landscape, small margins, and the
/// table headers repeated on every page.
const PRINT_CSS: &str =
    "@page { size: A4 landscape; margin: 10mm; } thead { display: table-header-group; }";

/// The report with the print rules, and a print request when `print` (列印 opens the dialog).
pub fn printable(html: &str, print: bool) -> String {
    let script = if print {
        "<script>window.addEventListener('load', () => window.print());</script>"
    } else {
        ""
    };
    html.replacen("</style>", &format!("{PRINT_CSS}</style>"), 1)
        .replacen("</body>", &format!("{script}</body>"), 1)
}

/// Write the report into `dir` (the per-user data folder, not the shared temp folder) where the
/// browser can open it.
pub fn write_report_file(dir: &Path, html: &str, print: bool, name: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.html"));
    std::fs::write(&path, printable(html, print))?;
    Ok(path)
}

fn windows_browsers() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (var, rest) in [
        (
            "ProgramFiles(x86)",
            r"Microsoft\Edge\Application\msedge.exe",
        ),
        ("ProgramFiles", r"Microsoft\Edge\Application\msedge.exe"),
        ("ProgramFiles", r"Google\Chrome\Application\chrome.exe"),
        ("ProgramFiles(x86)", r"Google\Chrome\Application\chrome.exe"),
        ("LOCALAPPDATA", r"Google\Chrome\Application\chrome.exe"),
    ] {
        if let Some(base) = std::env::var_os(var) {
            out.push(PathBuf::from(base).join(rest));
        }
    }
    out
}

fn macos_browsers() -> Vec<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn path_browsers() -> Vec<PathBuf> {
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
    ];
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
                .collect()
        })
        .unwrap_or_default()
}

/// An installed Chromium-based browser that can print to PDF headlessly.
pub fn find_browser() -> Option<PathBuf> {
    let candidates = if cfg!(windows) {
        windows_browsers()
    } else if cfg!(target_os = "macos") {
        macos_browsers()
    } else {
        path_browsers()
    };
    candidates.into_iter().find(|p| p.is_file())
}

const PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// Print `html_file` to `pdf` with a headless browser using the throw-away profile folder
/// `profile` (removed afterwards); the error is shown under 匯出失敗.
pub fn html_to_pdf(
    browser: &Path,
    html_file: &Path,
    pdf: &Path,
    profile: &Path,
) -> Result<(), String> {
    let result = run_browser(browser, html_file, pdf, profile);
    let _ = std::fs::remove_dir_all(profile); // only a cache; a leftover is harmless
    result
}

/// Flags that keep a headless browser from waiting on anything but the page: first-run screens,
/// the macOS keychain (which stalls Chrome in a session nobody watches), extensions — the set
/// Puppeteer uses.
const BROWSER_FLAGS: [&str; 9] = [
    "--headless=new",
    "--disable-gpu",
    "--no-pdf-header-footer",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-dev-shm-usage",
    "--password-store=basic",
    "--use-mock-keychain",
];
const PDF_TAIL: u64 = 1024;

/// A finished PDF: written since `before` and ending with the `%%EOF` marker.
fn pdf_complete(pdf: &Path, before: Option<SystemTime>) -> bool {
    let Ok(meta) = std::fs::metadata(pdf) else {
        return false;
    };
    if meta.len() == 0 || meta.modified().ok() == before {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(pdf) else {
        return false;
    };
    let mut tail = Vec::new();
    let read = file
        .seek(SeekFrom::Start(meta.len().saturating_sub(PDF_TAIL)))
        .and_then(|_| file.read_to_end(&mut tail));
    read.is_ok() && tail.trim_ascii_end().ends_with(b"%%EOF")
}

fn run_browser(browser: &Path, html_file: &Path, pdf: &Path, profile: &Path) -> Result<(), String> {
    let cannot_write = || format!("無法寫入 {}（檔案是否已在其他程式開啟？）", pdf.display());
    let before = std::fs::metadata(pdf).and_then(|m| m.modified()).ok();
    let mut child = Command::new(browser)
        .args(BROWSER_FLAGS)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--print-to-pdf={}", pdf.display()))
        .arg(html_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("無法啟動 {}：{e}", browser.display()))?;
    let started = Instant::now();
    loop {
        if pdf_complete(pdf, before) {
            // Done, even if the browser has not exited yet (it may keep helpers running).
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait(); // reap it
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PDF_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait(); // reap it
                return Err("產生 PDF 逾時".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    if pdf_complete(pdf, before) {
        Ok(())
    } else {
        Err(cannot_write())
    }
}

// ------------------------------------------------------------------ main window

impl OpenDrsApp {
    /// The report's title: the file name, where the project came from, or 寫頻資料.
    pub fn report_title(&self) -> String {
        match self.doc.path().and_then(Path::file_stem) {
            Some(stem) => stem.to_string_lossy().into_owned(),
            None if !self.doc.source().is_empty() => self.doc.source().to_string(),
            None => String::new(),
        }
    }

    /// Where the report files for the browser go: the per-user data folder.
    fn print_dir(&self) -> PathBuf {
        self.data_dir().join("print")
    }

    /// The report of the current project, stamped now.
    pub fn report(&self) -> String {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        report_html(self.project(), self.codec, &self.report_title(), &stamp)
    }

    fn show_report(&mut self, title: &str, print: bool) -> bool {
        if !self.require_project(title) {
            return false;
        }
        let shown = write_report_file(&self.print_dir(), &self.report(), print, "OpenDRS-report")
            .and_then(|path| open_with_system(&path).map(|()| path));
        match shown {
            Ok(path) => {
                self.status = format!("已在瀏覽器開啟{title}內容：{}", path.display());
                true
            }
            Err(e) => {
                self.warn(&format!("{title}失敗"), e.to_string());
                false
            }
        }
    }

    /// 列印…: the report opens in the browser with its print dialog (A4 landscape).
    pub fn print_project(&mut self) -> bool {
        self.show_report("列印", true)
    }

    /// 預覽列印…: the report opens in the browser.
    pub fn print_preview(&mut self) -> bool {
        self.show_report("預覽列印", false)
    }

    /// 匯出 PDF…: ask for a file, then print the report to it with a headless browser.
    pub fn export_pdf(&mut self) -> bool {
        if !self.require_project("匯出 PDF") {
            return false;
        }
        let (folder, name) = match self.doc.path() {
            Some(path) => (
                path.parent().map(Path::to_path_buf),
                path.with_extension("pdf")
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
            ),
            None => (
                (!self.settings().last_dir.is_empty())
                    .then(|| PathBuf::from(&self.settings().last_dir)),
                None,
            ),
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title("匯出 PDF")
            .add_filter("PDF", &["pdf"])
            .set_file_name(name.unwrap_or_else(|| "寫頻資料.pdf".into()));
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        match dialog.save_file() {
            Some(path) => self.export_pdf_to(&path),
            None => false,
        }
    }

    /// Write the report as a PDF to `path`.
    pub fn export_pdf_to(&mut self, path: &Path) -> bool {
        let Some(browser) = find_browser() else {
            self.warn("匯出失敗", NO_BROWSER);
            return false;
        };
        let dir = self.print_dir();
        let written = write_report_file(&dir, &self.report(), false, "OpenDRS-pdf")
            .map_err(|e| e.to_string())
            .and_then(|html| html_to_pdf(&browser, &html, path, &dir.join("pdf-profile")));
        match written {
            Ok(()) => {
                self.status = format!("已匯出 PDF 到 {}", path.display());
                true
            }
            Err(message) => {
                self.warn("匯出失敗", message);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_is_complete_once_it_ends_with_eof() {
        let dir = std::env::temp_dir().join(format!("opendrs-pdf-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let pdf = dir.join("a.pdf");
        assert!(!pdf_complete(&pdf, None), "missing");
        std::fs::write(&pdf, b"%PDF-1.7\n1 0 obj").expect("write");
        assert!(!pdf_complete(&pdf, None), "still being written");
        std::fs::write(&pdf, b"%PDF-1.7\n1 0 obj\ntrailer\n%%EOF\n").expect("write");
        assert!(pdf_complete(&pdf, None));
        let stamp = std::fs::metadata(&pdf).and_then(|m| m.modified()).ok();
        assert!(
            !pdf_complete(&pdf, stamp),
            "an old file from before the export"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn print_rules_are_added_to_the_report() {
        let html = "<html><head><style>body{}</style></head><body>x</body></html>";
        let preview = printable(html, false);
        assert!(
            preview.contains("@page { size: A4 landscape;") && !preview.contains("window.print")
        );
        assert!(printable(html, true).contains("window.print()"));
    }
}
