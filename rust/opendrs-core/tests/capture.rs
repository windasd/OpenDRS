//! The 2026-10-08 USB capture of the legacy CPS reading and writing a real radio: the Rust client
//! must send the very same bytes and understand every reply (ports test_codeplug / test_radio_link).

mod common;

use common::{ReplayTransport, cap};
use opendrs_core::codeplug::{BLOCK, Counts, build_stream, parse_stream, with_synced_image};
use opendrs_core::f136;
use opendrs_core::radio_link::{RadioClient, byte_sum};
use opendrs_core::textcodec::NameCodec;

fn written() -> f136::Project {
    f136::load(common::fixtures().join("capture/written_project.F136")).expect("written project")
}

fn reply_counts(reply: &[u8]) -> (Counts, usize, usize) {
    let field = |i: usize| usize::from(u16::from_le_bytes([reply[12 + 2 * i], reply[13 + 2 * i]]));
    (
        Counts::new(field(0), field(1), field(2)),
        field(3),
        field(4),
    )
}

#[test]
fn block_formulas_match_what_the_radio_reported() {
    let (counts, p1, p2) = reply_counts(&cap("read_reply.bin"));
    assert_eq!(counts, Counts::new(4, 27, 360));
    assert_eq!(
        (counts.part1_blocks(), counts.part2_blocks(), p1, p2),
        (6, 1, 6, 1)
    );
    assert_eq!(counts.stream_size(), cap("read_stream.bin").len());
    assert_eq!(
        cap("write_cmd.bin")[15..19],
        (counts.total_blocks() as u32).to_be_bytes()
    );
}

#[test]
fn build_stream_reproduces_the_captured_write() {
    assert_eq!(
        build_stream(&written(), Some("10/8/2026 16:54")).unwrap(),
        cap("write_stream.bin")
    );
    let ours = build_stream(&written(), None).unwrap();
    let captured = cap("write_stream.bin");
    let diff: Vec<usize> = (0..captured.len())
        .filter(|&i| ours[i] != captured[i])
        .collect();
    assert!(!diff.is_empty() && diff.iter().all(|i| (0x60..0x70).contains(i)));
}

#[test]
fn parsed_read_reproduces_the_project() {
    let project = parse_stream(&cap("read_stream.bin"), Counts::new(4, 27, 360)).unwrap();
    let written = written();
    assert_eq!(project.contacts, written.contacts);
    assert_eq!(project.zones, written.zones);
    assert_eq!(project.channels, written.channels);
    assert_eq!((project.image.len(), written.image.len()), (11_000, 11_000));
    assert!(
        (0..11_000)
            .filter(|&i| project.image[i] != written.image[i])
            .all(|i| (0x60..0x70).contains(&i))
    );
    let test = &project.channels[9];
    assert_eq!(
        (
            test.name(NameCodec::Ascii).as_str(),
            test.rx_hz(),
            test.tx_hz()
        ),
        ("test", 439_137_500, 431_537_500)
    );
    let first = parse_stream(&cap("read1_stream.bin"), Counts::new(4, 26, 360)).unwrap();
    assert!(
        first.channels.len() == 26
            && first
                .channels
                .iter()
                .all(|c| c.name(NameCodec::Ascii) != "test")
    );
}

#[test]
fn read_then_write_round_trip_and_legacy_projects_are_consistent() {
    let stream = cap("read_stream.bin");
    assert_eq!(
        build_stream(
            &parse_stream(&stream, Counts::new(4, 27, 360)).unwrap(),
            None
        )
        .unwrap(),
        stream
    );
    for name in [
        "mmdvm_20241124.F136",
        "mmdvm_20241127.F136",
        "dh98vu_20250811.F136",
    ] {
        let project = f136::load(common::fixtures().join(name)).unwrap();
        assert_eq!(with_synced_image(&project), project, "{name}");
    }
}

fn read_exchanges() -> Vec<(Vec<u8>, Vec<u8>)> {
    let stream = cap("read_stream.bin");
    let mut ex = vec![(cap("read_cmd.bin"), cap("read_reply.bin"))];
    ex.extend(stream.chunks(BLOCK).map(|b| (b"Read".to_vec(), b.to_vec())));
    if let Some(last) = ex.last_mut() {
        last.1.extend(cap("read_checksum.bin"));
    }
    ex
}

fn write_exchanges() -> Vec<(Vec<u8>, Vec<u8>)> {
    let stream = cap("write_stream.bin");
    let tail = cap("write_radio_tail.bin");
    let acks = tail[..30].windows(5).filter(|w| *w == b"Write").count();
    let blocks: Vec<&[u8]> = stream.chunks(BLOCK).collect();
    let mut ex = vec![(cap("write_cmd.bin"), cap("write_reply.bin"))];
    ex.extend(
        blocks[..blocks.len() - 1]
            .iter()
            .map(|b| (b.to_vec(), b"Write".to_vec())),
    );
    ex.push((blocks[blocks.len() - 1].to_vec(), tail[5 * acks..].to_vec()));
    ex
}

#[test]
fn replayed_read_matches_the_legacy_cps_byte_for_byte() {
    let mut t = ReplayTransport::new(read_exchanges());
    let mut progress = Vec::new();
    let result =
        RadioClient::with_progress(&mut t, |d, n, s: &str| progress.push((d, n, s.to_string())))
            .read_codeplug()
            .unwrap();
    assert!(t.finished());
    assert_eq!(result.counts, Counts::new(4, 27, 360));
    assert_eq!(result.stream, cap("read_stream.bin"));
    assert_eq!(
        (
            result.identity.model.as_str(),
            result.identity.band.as_str()
        ),
        ("DRS-8500", "136-480")
    );
    assert!(result.identity.firmware.starts_with("308E.BBKTARSCB.003"));
    assert_eq!(progress.last().unwrap(), &(7, 7, "讀取完成".to_string()));
    assert_eq!(result.project.channels, written().channels);
}

#[test]
fn replayed_write_matches_the_legacy_cps_byte_for_byte() {
    let mut t = ReplayTransport::new(write_exchanges());
    let result = RadioClient::new(&mut t)
        .write_codeplug(&written(), Some("10/8/2026 16:54"), None)
        .unwrap();
    assert!(t.finished());
    assert_eq!(
        t.sent,
        [cap("write_cmd.bin"), cap("write_stream.bin")].concat()
    );
    assert_eq!(result.checksum, byte_sum(&cap("write_stream.bin")));
    assert!(result.radio_log.contains("Manufacture:TC58"));
}
