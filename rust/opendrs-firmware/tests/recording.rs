//! Call-recording download: frames vs Wav.exe, list/read against a simulated radio, audio, and
//! keeping downloads on disk (port of tests/test_recording.py and of the non-GUI parts of
//! tests/test_recording_dialog.py).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Chunked, fixtures, temp_dir};
use opendrs_core::radio_link::Transport;
use opendrs_firmware::recording::{
    BLOCK, DECODE_TIMEOUT, MISSING_DECODER_NOTE, NO_DECODER_NOTE, RecordClient, RecordInfo,
    RecordingError, checksum16, decode_ambe, default_ambep2, list_request, record_read_command,
    save_recording, to_bit_stream, write_wav,
};
use opendrs_firmware::recording_sim::{SimulatedRecorder, demo_records};

const FAST: Duration = Duration::from_millis(50);

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Every byte Wav.exe 1.00.0072 wrote while listing 3 records and downloading them, captured by
/// running it against a simulated radio.
fn wav_exe_tx() -> Vec<u8> {
    std::fs::read(fixtures().join("recording/wav_exe_tx.bin")).expect("fixture")
}

fn client<T: Transport>(radio: &mut T) -> RecordClient<'_, T> {
    let mut client = RecordClient::new(radio);
    client.list_timeout = FAST;
    client.reply_timeout = FAST;
    client.block_timeout = FAST;
    client.checksum_timeout = FAST;
    client
}

fn message(error: RecordingError) -> String {
    error.to_string()
}

fn recorder(configure: impl FnOnce(&mut SimulatedRecorder)) -> SimulatedRecorder {
    let mut x = SimulatedRecorder::default();
    configure(&mut x);
    x
}

#[test]
fn frames_match_wav_exe() {
    let tx = wav_exe_tx();
    assert_eq!(
        list_request(100),
        [
            0x68, 0x58, 0xff, 0x01, 0x86, 0x41, 0x02, 0x00, 0x00, 0x64, 0x10
        ]
    );
    for n in [100, 99, 98, 97, 96] {
        assert!(find(&tx, &list_request(n)).is_some(), "{n}");
    }
    assert_eq!(
        record_read_command(1, 3),
        b"Record Read\x00\x00\x00\x01\x00\x00\x00\x03"
    );
    for (info, _) in demo_records() {
        let number = u32::try_from(info.number).unwrap();
        let command = record_read_command(number, info.pages_field());
        assert!(find(&tx, &command).is_some(), "{number}");
    }
    let frame = [
        0x68, 0x58, 0xff, 0x01, 0x00, 0x00, 0x02, 0x00, 0x00, 0x64, 0x10,
    ];
    assert_eq!(checksum16(&frame), 0x8641);
}

#[test]
fn record_line() {
    let info =
        RecordInfo::parse(b"junk#12#40#S#09998765#8433#2025/06/07 12:34Record Read").unwrap();
    let expected = RecordInfo {
        number: 12,
        total: 40,
        direction: 'S',
        call_id: 9_998_765,
        length: 8433,
        time: "2025/06/07 12:34".into(),
    };
    assert_eq!(info, expected);
    assert_eq!(info.line(), b"#12#40#S#09998765#8433#2025/06/07 12:34");
    assert_eq!(
        (info.blocks(), info.pages_field(), info.stem()),
        (3, 3, "20250607_1234_12_S09998765".to_owned())
    );
    assert_eq!((info.seconds() * 100.0).round() / 100.0, 18.74);
    let exact = RecordInfo {
        number: 1,
        total: 1,
        direction: 'R',
        call_id: 1,
        length: BLOCK as u64,
        time: "2025/01/01 00:00".into(),
    };
    // Wav.exe's off-by-one, ignored by the radio
    assert_eq!((exact.blocks(), exact.pages_field()), (1, 2));
    assert_eq!(RecordInfo::parse(b"#1#2#X#1#2#2025/01/01 00:00"), None);
}

#[test]
fn list_and_read_against_simulated_radio() {
    let mut radio = SimulatedRecorder::default();
    let records = radio.records.clone();
    let mut c = client(&mut radio);
    let infos = c.list_records(100).unwrap();
    assert_eq!(
        infos.iter().map(|i| i.number).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        infos,
        records.iter().map(|r| r.0.clone()).collect::<Vec<_>>()
    );
    for (info, data) in &records {
        let number = u32::try_from(info.number).unwrap();
        let (header, got) = c.read_record(number, info.pages_field()).unwrap();
        assert_eq!(&header, info);
        assert_eq!(&got, data);
    }
}

#[test]
fn list_stops_at_max_and_reports_unsupported_firmware() {
    let mut radio = SimulatedRecorder::default();
    assert_eq!(client(&mut radio).list_records(2).unwrap().len(), 2);
    let mut radio = recorder(|x| x.supports_list = false);
    let err = message(client(&mut radio).list_records(100).unwrap_err());
    assert!(err.contains("不支援從電腦讀取錄音"), "{err}");
}

#[test]
fn read_works_without_the_list_command() {
    let mut radio = recorder(|x| x.supports_list = false);
    let expected = radio.records[0].1.clone();
    let (header, data) = client(&mut radio).read_record(1, 1).unwrap();
    assert_eq!(header.total, 3);
    assert_eq!(data, expected);
}

#[test]
fn read_failures() {
    let cases = [
        (
            recorder(|x| x.supports_record_read = false),
            "沒有回傳第 1 筆",
        ),
        (recorder(|x| x.corrupt_checksum = true), "校驗和不符"),
    ];
    for (mut radio, text) in cases {
        let err = message(client(&mut radio).read_record(1, 1).unwrap_err());
        assert!(err.contains(text), "{err}");
    }
}

#[test]
fn missing_record_is_reported() {
    let mut radio = SimulatedRecorder::default();
    let err = message(client(&mut radio).read_record(9, 1).unwrap_err());
    assert!(err.contains("沒有回傳第 9 筆"), "{err}");
}

#[test]
fn bit_stream_is_msb_first_whole_frames() {
    let data = [&[0x80, 0x01][..], &[0; 7], &[0xff]].concat();
    let bits = to_bit_stream(&data);
    assert_eq!(bits.len(), 72);
    assert_eq!(bits[..16], [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
}

/// (channels, sample width, frame rate, frames) from a WAV header.
fn wav_params(path: &Path) -> (u16, u16, u32, u32) {
    let wav = std::fs::read(path).unwrap();
    let u16_at = |at: usize| u16::from_le_bytes([wav[at], wav[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([wav[at], wav[at + 1], wav[at + 2], wav[at + 3]]);
    assert_eq!(
        (&wav[..4], &wav[8..16], &wav[36..40]),
        (&b"RIFF"[..], &b"WAVEfmt "[..], &b"data"[..])
    );
    let block_align = u32::from(u16_at(32));
    (
        u16_at(22),
        u16_at(34) / 8,
        u32_at(24),
        u32_at(40) / block_align,
    )
}

#[test]
fn write_wav_header() {
    let dir = temp_dir("write_wav");
    let pcm: Vec<u8> = 1000i16.to_le_bytes().repeat(160);
    let path = write_wav(&dir.join("a.wav"), &pcm).unwrap();
    assert_eq!(wav_params(&path), (1, 2, 8000, 160));
}

#[test]
fn decode_reports_missing_ambep2() {
    let dir = temp_dir("missing_ambep2");
    let err =
        message(decode_ambe(b"xxxxxxxxx", &dir.join("ambep2.exe"), DECODE_TIMEOUT).unwrap_err());
    assert!(err.contains("找不到 ambep2"), "{err}");
}

/// The vendor decoder, when installed where the vendor puts it (Windows only).
fn vendor_ambep2() -> Option<PathBuf> {
    let found = default_ambep2().filter(|p| p.is_file());
    if found.is_none() {
        eprintln!("skipped: vendor ambep2.exe not installed");
    }
    found
}

#[test]
fn decode_with_vendor_ambep2() {
    let Some(exe) = vendor_ambep2() else {
        return;
    };
    let data = &demo_records()[0].1;
    let pcm = decode_ambe(data, &exe, DECODE_TIMEOUT).unwrap();
    assert_eq!(pcm.len(), data.len() / 9 * 320); // 160 samples of 16 bit per 9-byte frame
}

/// `wav_exe_block.bit` is what Wav.exe wrote for one 4050-byte block (demo record 2: 3000 bytes
/// of data + 0xFF padding) before handing it to ambep2.
#[test]
fn bit_stream_matches_wav_exe() {
    let (info, data) = &demo_records()[1];
    let mut block = data.clone();
    block.resize(BLOCK, 0xFF);
    assert_eq!(info.length, 3000);
    let expected = std::fs::read(fixtures().join("recording/wav_exe_block.bit")).unwrap();
    assert_eq!(to_bit_stream(&block), expected);
}

#[test]
fn replies_split_into_small_reads() {
    let mut chunked = Chunked::new(SimulatedRecorder::default());
    let records = chunked.inner.records.clone();
    let mut c = RecordClient::new(&mut chunked);
    c.block_timeout = Duration::from_secs(30);
    let infos = c.list_records(100).unwrap();
    assert_eq!(
        infos,
        records.iter().map(|r| r.0.clone()).collect::<Vec<_>>()
    );
    let (header, data) = c.read_record(1, 3).unwrap();
    assert_eq!(header.number, 1);
    assert_eq!(data, records[0].1);
}

#[test]
fn cancel_list_and_read() {
    let mut radio = SimulatedRecorder::default();
    let err = RecordClient::new(&mut radio)
        .with_cancel(|| true)
        .list_records(100)
        .unwrap_err();
    assert!(message(err).contains("已停止"));
    let mut radio = SimulatedRecorder::default();
    let mut calls = [false, true].into_iter();
    let err = RecordClient::new(&mut radio)
        .with_cancel(move || calls.next().unwrap_or(true))
        .read_record(1, 3)
        .unwrap_err();
    assert!(message(err).contains("已停止"));
}

#[test]
fn simulated_radio_lists_again_from_the_top() {
    let mut radio = SimulatedRecorder::default();
    let mut c = client(&mut radio);
    assert_eq!(c.list_records(100).unwrap().len(), 3);
    let numbers: Vec<u64> = c
        .list_records(2)
        .unwrap()
        .iter()
        .map(|i| i.number)
        .collect();
    assert_eq!(numbers, [1, 2]);
}

/// A stand-in decoder script (the Python test monkeypatches `subprocess.run` instead).
fn fake_decoder(dir: &Path, name: &str, windows: &str, unix: &str) -> PathBuf {
    let (file, body) = if cfg!(windows) {
        (format!("{name}.cmd"), format!("@echo off\r\n{windows}\r\n"))
    } else {
        (name.to_owned(), format!("#!/bin/sh\n{unix}\n"))
    };
    let path = dir.join(file);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn decode_failure_is_reported() {
    let dir = temp_dir("decode_failure");
    let failing = fake_decoder(
        &dir,
        "failing",
        "echo header 1>&2\r\necho Can't open input data file rec.bit 1>&2\r\nexit /b 1",
        "echo header >&2\necho \"Can't open input data file rec.bit\" >&2\nexit 1",
    );
    let err = message(decode_ambe(b"xxxxxxxxx", &failing, DECODE_TIMEOUT).unwrap_err());
    assert!(err.contains("結束碼 1"), "{err}");
    assert!(err.ends_with("Can't open input data file rec.bit"), "{err}");

    let no_output = fake_decoder(&dir, "no_output", "exit /b 0", "exit 0");
    let err = message(decode_ambe(b"xxxxxxxxx", &no_output, DECODE_TIMEOUT).unwrap_err());
    assert!(err.contains("沒有產生輸出"), "{err}");
}

#[test]
fn decoder_gets_the_bit_stream_and_its_output_is_returned() {
    let dir = temp_dir("decode_copy");
    let copying = fake_decoder(&dir, "copying", "copy /b %2 %3 >nul", "cp \"$2\" \"$3\"");
    let data = &demo_records()[1].1;
    let pcm = decode_ambe(data, &copying, DECODE_TIMEOUT).unwrap();
    assert_eq!(pcm, to_bit_stream(data));
}

#[test]
fn slow_decoder_times_out() {
    let dir = temp_dir("decode_slow");
    let slow = fake_decoder(&dir, "slow", "ping -n 3 127.0.0.1 >nul", "sleep 2");
    let err = message(decode_ambe(b"xxxxxxxxx", &slow, Duration::from_millis(300)).unwrap_err());
    assert!(err.starts_with("ambep2.exe 執行失敗：Command '["), "{err}");
    assert!(
        err.ends_with("', '-dec', 'rec.bit', 'rec.dat']' timed out after 0.3 seconds"),
        "{err}"
    );
}

#[test]
fn list_asks_for_record_numbers_like_the_firmware_handler() {
    // The firmware's handler answers the record whose number is in the payload, and accepts a
    // frame whose one's-complement sum over all bytes is 0.
    let mut radio = SimulatedRecorder::default();
    client(&mut radio).list_records(100).unwrap();
    let frames: Vec<Vec<u8>> = radio.sent.chunks(11).map(<[u8]>::to_vec).collect();
    assert_eq!(frames, (1..=3).map(list_request).collect::<Vec<_>>());
    assert!(frames.iter().all(|f| checksum16(f) == 0));
}

// ---------------------------------------------------------------------- saving (recording dialog)

#[test]
fn download_keeps_raw_data_without_a_decoder() {
    let dir = temp_dir("save_no_decoder");
    let (info, data) = &demo_records()[0];
    let saved = save_recording(&dir.join("rec"), info, data, None).unwrap();
    assert_eq!(saved.raw, dir.join("rec/20250607_1234_1_R09998765.ambe"));
    assert_eq!(std::fs::read(&saved.raw).unwrap(), *data);
    assert_eq!((saved.wav, saved.note.as_str()), (None, NO_DECODER_NOTE));
    let missing = save_recording(&dir.join("rec"), info, data, Some(&dir.join("missing.exe")));
    let missing = missing.unwrap();
    assert!(missing.note.contains("找不到 ambep2") && missing.note == MISSING_DECODER_NOTE);
    assert_eq!(missing.raw, saved.raw); // the same recording again: same file
}

#[test]
fn names_do_not_collide() {
    let dir = temp_dir("save_collide");
    let (info, data) = &demo_records()[0];
    save_recording(&dir, info, data, None).unwrap();
    // the radio now holds a different call under record 1 (same time, direction and ID)
    let other: Vec<u8> = data.iter().rev().copied().collect();
    let saved = save_recording(&dir, info, &other, None).unwrap();
    assert_eq!(
        saved.raw.file_name().unwrap(),
        "20250607_1234_1_R09998765_2.ambe"
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "20250607_1234_1_R09998765.ambe",
            "20250607_1234_1_R09998765_2.ambe"
        ]
    );
}

#[test]
fn download_reports_a_failed_conversion_as_a_note() {
    let dir = temp_dir("save_failed_decoder");
    let failing = fake_decoder(&dir, "failing", "exit /b 3", "exit 3");
    let (info, data) = &demo_records()[1];
    let saved = save_recording(&dir.join("out"), info, data, Some(&failing)).unwrap();
    assert!(
        saved.wav.is_none() && saved.note.contains("結束碼 3"),
        "{}",
        saved.note
    );
    assert!(saved.raw.is_file());
}

#[test]
fn download_decodes_to_wav() {
    let Some(exe) = vendor_ambep2() else {
        return;
    };
    let dir = temp_dir("save_decode");
    let (info, data) = &demo_records()[0];
    let saved = save_recording(&dir, info, data, Some(&exe)).unwrap();
    let wav = saved.wav.expect("decoded");
    assert_eq!(wav.file_name().unwrap(), "20250607_1234_1_R09998765.wav");
    let (_, _, rate, frames) = wav_params(&wav);
    assert_eq!((rate, frames), (8000, 8433 / 9 * 160));
}
