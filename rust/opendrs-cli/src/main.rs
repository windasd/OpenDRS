//! `opendrs-rs`: command-line front end of the Rust port of OpenDRS.
//!
//! Inspect `.F136` projects, and read / write the radio over the programming cable (or the
//! simulated radio, `--port sim`). Writing follows the same safety rules as the Python GUI: read
//! and back up the radio first, refuse on a model / band mismatch, and only with `--yes`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use opendrs_core::codeplug::{self, Counts, DATE_FORMAT};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_link::{
    self, RadioClient, RadioIdentity, ReadResult, SerialTransport, Transport, identity_mismatch,
};
use opendrs_core::radio_sim::SimulatedRadio;
use opendrs_core::records::hz_to_mhz;
use opendrs_core::textcodec::{self, NameCodec};

#[derive(Parser)]
#[command(
    name = "opendrs-rs",
    version,
    about = "OpenDRS（Rust 版）：DR-8500UV／DH-98VU 寫頻工具"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    Big5,
    Gbk,
    Ascii,
}

impl From<Codec> for NameCodec {
    fn from(codec: Codec) -> Self {
        match codec {
            Codec::Big5 => NameCodec::Big5,
            Codec::Gbk => NameCodec::Gbk,
            Codec::Ascii => NameCodec::Ascii,
        }
    }
}

#[derive(clap::Args)]
struct RadioArgs {
    /// 寫頻線連接埠（例如 COM4），或 `sim` 使用模擬對講機
    #[arg(long)]
    port: String,
    /// 模擬對講機的初始內容（.F136；只用於 --port sim）
    #[arg(long)]
    sim_from: Option<PathBuf>,
    /// 備份資料夾（預設為資料夾內的 backups：Windows %APPDATA%\OpenDRS、
    /// macOS ~/Library/Application Support/OpenDRS、Linux ~/.config/OpenDRS）
    #[arg(long)]
    backup_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// 顯示專案的裝置資訊、數量與問題
    Info { file: PathBuf },
    /// 列出聯絡人
    Contacts {
        file: PathBuf,
        #[arg(long, value_enum)]
        codec: Option<Codec>,
    },
    /// 依分區列出頻道
    Channels {
        file: PathBuf,
        #[arg(long, value_enum)]
        codec: Option<Codec>,
    },
    /// 檢查：重新輸出是否與原檔逐位元組相同、能否寫入對講機
    Check { file: PathBuf },
    /// 列出序列埠（寫頻線排在前面）
    Ports,
    /// 讀取對講機並存成 .F136（同時保留備份）
    Read {
        #[command(flatten)]
        radio: RadioArgs,
        /// 輸出的專案檔
        #[arg(short, long)]
        out: PathBuf,
    },
    /// 寫入對講機：先讀取並備份，機型或頻段不符會中止；需加 --yes
    Write {
        #[command(flatten)]
        radio: RadioArgs,
        file: PathBuf,
        /// 確認要寫入
        #[arg(long)]
        yes: bool,
    },
    /// 校正對講機時鐘：讀取後原樣寫回、只更新寫入時間；需加 --yes
    Clock {
        #[command(flatten)]
        radio: RadioArgs,
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Info { file } => info(&file),
        Command::Contacts { file, codec } => contacts(&file, codec),
        Command::Channels { file, codec } => channels(&file, codec),
        Command::Check { file } => check(&file),
        Command::Ports => ports(),
        Command::Read { radio, out } => read(&radio, &out),
        Command::Write { radio, file, yes } => write(&radio, &file, yes),
        Command::Clock { radio, yes } => clock(&radio, yes),
    }
}

fn load(path: &Path) -> Result<Project> {
    f136::load(path).with_context(|| format!("無法讀取 {}", path.display()))
}

fn name_fields(project: &Project) -> Vec<&[u8]> {
    project
        .contacts
        .iter()
        .map(|c| c.name_field())
        .chain(project.zones.iter().map(|z| z.name_field()))
        .chain(project.channels.iter().map(|c| c.name_field()))
        .collect()
}

/// The codec asked for, else the one the names look like, else Big5.
fn codec_for(project: &Project, asked: Option<Codec>) -> NameCodec {
    asked
        .map(NameCodec::from)
        .or_else(|| textcodec::suggest_codec(&name_fields(project)))
        .unwrap_or(NameCodec::Big5)
}

fn info(path: &Path) -> Result<()> {
    let project = load(path)?;
    let dev = project.device();
    let or_dash = |s: &str| {
        if s.is_empty() {
            "—".to_string()
        } else {
            s.to_string()
        }
    };
    println!(
        "機型　　{}（型號 {}）",
        or_dash(&dev.model),
        or_dash(&dev.model_number)
    );
    println!("頻段　　{}", or_dash(&dev.band_label));
    println!("韌體　　{} {}", or_dash(&dev.firmware), dev.firmware_date);
    println!(
        "別名　　{}　DMR ID {}",
        or_dash(&dev.alias),
        dev.dmr_id.map_or("—".into(), |id| id.to_string())
    );
    println!("寫入時間 {}", or_dash(&dev.write_date));
    println!(
        "分區 {}、頻道 {}、聯絡人 {}　名稱編碼 {}",
        project.zones.len(),
        project.channels.len(),
        project.contacts.len(),
        codec_for(&project, None).label()
    );
    let problems = project.problems();
    if problems.is_empty() {
        println!("沒有發現問題。");
    } else {
        println!("問題：\n  {}", problems.join("\n  "));
    }
    Ok(())
}

fn contacts(path: &Path, codec: Option<Codec>) -> Result<()> {
    let project = load(path)?;
    let codec = codec_for(&project, codec);
    for c in &project.contacts {
        println!(
            "{:>4}  {:<6}  {:<12}  {}",
            c.index(),
            c.kind_label(),
            c.name(codec),
            c.call_id()
        );
    }
    Ok(())
}

fn channels(path: &Path, codec: Option<Codec>) -> Result<()> {
    let project = load(path)?;
    let codec = codec_for(&project, codec);
    let mut row = 0;
    for (z, zone) in project.zones.iter().enumerate() {
        println!(
            "分區 {}：{}（{} 個頻道）",
            z + 1,
            zone.name(codec),
            zone.channel_count()
        );
        for position in 1..=usize::from(zone.channel_count()) {
            let Some(ch) = project.channels.get(row) else {
                break;
            };
            row += 1;
            println!(
                "  {:>3}  {:<12}  RX {}  TX {}  {}",
                position,
                ch.name(codec),
                hz_to_mhz(ch.rx_hz()),
                hz_to_mhz(ch.tx_hz()),
                ch.mode_label()
            );
        }
    }
    Ok(())
}

fn check(path: &Path) -> Result<()> {
    let original = std::fs::read(path).with_context(|| format!("無法讀取 {}", path.display()))?;
    let project = f136::parse(&original)?;
    let same = f136::serialize(&project) == original;
    println!(
        "重新輸出與原檔{}",
        if same {
            "逐位元組相同"
        } else {
            "不同（會同步設定區內的聯絡人與計數）"
        }
    );
    let reasons = codeplug::check_writable(&project);
    if reasons.is_empty() {
        let stream = codeplug::build_stream(&project, None)?;
        println!(
            "可以寫入對講機：{} 個區塊（{} bytes）",
            Counts::of(&project).total_blocks(),
            stream.len()
        );
    } else {
        println!("不能寫入對講機：\n  {}", reasons.join("\n  "));
    }
    Ok(())
}

fn ports() -> Result<()> {
    let ports = radio_link::list_ports()?;
    if ports.is_empty() {
        println!("找不到序列埠：請確認寫頻線已插入並安裝驅動程式。");
    }
    for p in ports {
        println!(
            "{}  {}{}",
            p.device,
            p.description,
            if p.is_programming_cable {
                "（寫頻線）"
            } else {
                ""
            }
        );
    }
    Ok(())
}

// ------------------------------------------------------------------ radio

fn open_transport(args: &RadioArgs) -> Result<Box<dyn Transport>> {
    if args.port.eq_ignore_ascii_case("sim") {
        let radio = match &args.sim_from {
            Some(path) => {
                let project = load(path)?;
                let stream = codeplug::build_stream(&project, None)
                    .context("模擬對講機的內容無法組成傳輸資料")?;
                SimulatedRadio::with_stream(&stream, Counts::of(&project))
            }
            None => SimulatedRadio::default(),
        };
        return Ok(Box::new(radio));
    }
    Ok(Box::new(SerialTransport::open(&args.port)?))
}

fn progress(done: usize, total: usize, text: &str) {
    eprint!("\r{text}（{done}/{total}）          ");
    if done == total {
        eprintln!();
    }
    let _ = std::io::stderr().flush(); // progress display only
}

fn backup_dir(args: &RadioArgs) -> PathBuf {
    args.backup_dir
        .clone()
        .unwrap_or_else(opendrs_core::paths::backups_dir)
}

fn read_radio(
    transport: &mut dyn Transport,
    args: &RadioArgs,
    kind: &str,
) -> Result<(ReadResult, PathBuf)> {
    let result = RadioClient::with_progress(transport, progress).read_codeplug()?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let backup = radio_link::save_backup(&result, &backup_dir(args), kind, &stamp)
        .context("無法保存備份")?;
    eprintln!(
        "已讀取：{}　備份：{}",
        result.identity.summary(),
        backup.display()
    );
    Ok((result, backup))
}

fn read(args: &RadioArgs, out: &Path) -> Result<()> {
    let mut transport = open_transport(args)?;
    let (result, _) = read_radio(transport.as_mut(), args, "read")?;
    std::fs::write(out, f136::serialize(&result.project))
        .with_context(|| format!("無法寫入 {}", out.display()))?;
    println!("已存成 {}", out.display());
    Ok(())
}

fn write_project(transport: &mut dyn Transport, project: &Project) -> Result<String> {
    let date_text = chrono::Local::now().format(DATE_FORMAT).to_string();
    let check = |identity: &RadioIdentity| identity_mismatch(project, identity);
    let result = RadioClient::with_progress(transport, progress).write_codeplug(
        project,
        Some(&date_text),
        Some(&check),
    )?;
    Ok(format!(
        "寫入完成，對講機確認校驗和 0x{:08X}；對講機時鐘已設為 {date_text}。",
        result.checksum
    ))
}

fn write(args: &RadioArgs, file: &Path, yes: bool) -> Result<()> {
    let project = load(file)?;
    let reasons = codeplug::check_writable(&project);
    if !reasons.is_empty() {
        bail!("無法寫入：\n{}", reasons.join("\n"));
    }
    if !yes {
        bail!("寫入會覆蓋對講機目前的設定。確認無誤後請加上 --yes 再執行。");
    }
    let mut transport = open_transport(args)?;
    eprintln!("寫入前先讀取對講機目前的設定並備份…");
    let (before, _) = read_radio(transport.as_mut(), args, "before_write")?;
    if let Some(reason) = identity_mismatch(&project, &before.identity) {
        bail!("已中止，未寫入任何資料：{reason}");
    }
    std::thread::sleep(std::time::Duration::from_millis(
        if args.port.eq_ignore_ascii_case("sim") {
            0
        } else {
            1500
        },
    ));
    println!("{}", write_project(transport.as_mut(), &project)?);
    Ok(())
}

fn clock(args: &RadioArgs, yes: bool) -> Result<()> {
    if !yes {
        bail!("校正時鐘會把讀到的設定原樣寫回對講機。確認後請加上 --yes 再執行。");
    }
    let mut transport = open_transport(args)?;
    let (before, _) = read_radio(transport.as_mut(), args, "before_write")?;
    let project = radio_link::clock_write_project(&before)?;
    std::thread::sleep(std::time::Duration::from_millis(
        if args.port.eq_ignore_ascii_case("sim") {
            0
        } else {
            1500
        },
    ));
    println!("{}", write_project(transport.as_mut(), &project)?);
    Ok(())
}
