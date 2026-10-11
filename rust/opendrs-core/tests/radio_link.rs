//! Protocol failure paths and backups of `radio_link` (ports the remaining non-serial tests of
//! test_radio_link.py and test_save_backup_names of test_radio_panel.py).

mod common;

use std::time::Duration;

use common::cap;
use opendrs_core::codeplug::{BLOCK, Counts};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_link::{
    RadioClient, RadioError, RadioIdentity, Transport, identity_mismatch, save_backup,
};
use opendrs_core::radio_sim::SimulatedRadio;

fn radio() -> SimulatedRadio {
    SimulatedRadio::with_stream(&cap("read_stream.bin"), Counts::new(4, 27, 360))
}

fn written() -> Project {
    f136::load(common::fixtures().join("capture/written_project.F136")).expect("written project")
}

/// Answers with fixed bytes whatever the PC sends, and records what it sent.
struct Garbage {
    reply: Vec<u8>,
    sent: Vec<u8>,
}

impl Garbage {
    fn new(reply: &[u8]) -> Self {
        Garbage {
            reply: reply.to_vec(),
            sent: Vec::new(),
        }
    }
}

impl Transport for Garbage {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.sent.extend_from_slice(data);
        Ok(())
    }
    fn read_exact(&mut self, n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        if self.reply.len() < n {
            return Err(RadioError::Timeout {
                message: "x".into(),
                received: self.reply.len(),
            });
        }
        Ok(self.reply.drain(..n).collect())
    }
    fn read_idle(&mut self, _idle: Duration, _max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        Ok(Vec::new())
    }
    fn reset_input(&mut self) -> Result<(), RadioError> {
        Ok(())
    }
}

fn read_reply(fields: [u16; 5]) -> Vec<u8> {
    let mut reply = b"  Read_2M_\0\0".to_vec();
    for f in fields {
        reply.extend_from_slice(&f.to_le_bytes());
    }
    reply
}

#[test]
fn a_wrong_reply_prefix_is_reported() {
    let err = RadioClient::new(&mut Garbage::new(&[b'X'; 40]))
        .read_codeplug()
        .unwrap_err();
    assert!(err.to_string().contains("回應不符"), "{err}");
    let err = RadioClient::new(&mut Garbage::new(&[b'X'; 40]))
        .write_codeplug(&written(), None, None)
        .unwrap_err();
    assert!(err.to_string().contains("回應不符"), "{err}");
}

#[test]
fn inconsistent_block_counts_in_the_reply_stop_the_read() {
    let err = RadioClient::new(&mut Garbage::new(&read_reply([4, 27, 360, 5, 1])))
        .read_codeplug()
        .unwrap_err();
    assert!(err.to_string().contains("區塊數"), "{err}");
}

#[test]
fn a_missing_checksum_tag_is_reported() {
    let reply = [
        read_reply([1, 1, 0, 3, 1]),
        vec![0; 4 * BLOCK],
        b"NOPE".to_vec(),
        vec![0; 9],
    ]
    .concat();
    let err = RadioClient::new(&mut Garbage::new(&reply))
        .read_codeplug()
        .unwrap_err();
    assert!(err.to_string().contains("校驗碼"), "{err}");
}

/// The simulated radio with one bit of the first block flipped after its sum was taken.
struct Corrupting {
    radio: SimulatedRadio,
    flipped: bool,
}

impl Transport for Corrupting {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.radio.write(data)
    }
    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        let mut data = self.radio.read_exact(n, timeout)?;
        if n == BLOCK && !self.flipped {
            data[0] ^= 0x01;
            self.flipped = true;
        }
        Ok(data)
    }
    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        self.radio.read_idle(idle, max_wait)
    }
    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.radio.reset_input()
    }
}

#[test]
fn a_bad_read_checksum_is_reported() {
    let mut line = Corrupting {
        radio: radio(),
        flipped: false,
    };
    let err = RadioClient::new(&mut line).read_codeplug().unwrap_err();
    assert!(err.to_string().contains("校驗和不符"), "{err}");
}

#[test]
fn an_unwritable_project_is_refused_before_connecting() {
    let mut line = Garbage::new(b"");
    let err = RadioClient::new(&mut line)
        .write_codeplug(&Project::default(), None, None)
        .unwrap_err();
    assert!(err.to_string().contains("無法寫入"), "{err}");
    assert!(line.sent.is_empty());
}

#[test]
fn identity_parsing_variants() {
    let ident = RadioIdentity::parse(&cap("read_reply.bin")[22..]);
    assert_eq!(
        (ident.model.as_str(), ident.band.as_str()),
        ("DRS-8500", "136-480")
    );
    let blank = RadioIdentity::parse(b"\0\0");
    assert!(blank.model.is_empty() && blank.summary().contains("未知機型"));
}

#[test]
fn identity_mismatch_rules_for_the_captured_project() {
    let project = written();
    let ok = RadioIdentity::parse(b"308E\0+DRS-8500+136-480MHZ+");
    assert_eq!(identity_mismatch(&project, &ok), None);
    let other = |model: &str, band: &str| RadioIdentity {
        model: model.into(),
        band: band.into(),
        ..ok.clone()
    };
    let reason = |ident: RadioIdentity| identity_mismatch(&project, &ident).unwrap_or_default();
    assert!(reason(other("DRS-8600", "136-480")).contains("機型"));
    assert!(reason(other("DRS-8500", "400-480")).contains("頻段"));
    assert!(reason(other("", "136-480")).contains("無法辨識"));
}

#[test]
fn identity_mismatch_with_the_band_widened_test_firmware() {
    // That firmware reports +136-520MHZ+ and writes F136-520 at settings +0x50 on a read.
    let widened = RadioIdentity::parse(b"308E.BBKTARSCB.003\0+DRS-8500+136-520MHZ+\0");
    assert_eq!(widened.band, "136-520");
    let old = written(); // read from the vendor firmware: F136-480
    let reason = identity_mismatch(&old, &widened).unwrap_or_default();
    assert!(
        reason.contains("136-480") && reason.contains("136-520"),
        "{reason}"
    );
    let mut image = old.image.clone();
    image[0x50..0x58].copy_from_slice(b"F136-520");
    let reread = Project { image, ..old };
    assert_eq!(reread.device().band_label, "F136-520");
    assert_eq!(identity_mismatch(&reread, &widened), None);
}

#[test]
fn backups_are_named_by_time_kind_and_model() {
    let dir = std::env::temp_dir().join(format!("opendrs-link-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut result = RadioClient::new(&mut radio()).read_codeplug().unwrap();
    let path = save_backup(&result, &dir.join("b"), "read", "20261008_180000").unwrap();
    assert_eq!(
        path.file_name().unwrap().to_str(),
        Some("20261008_180000_read_DRS-8500.F136")
    );
    assert_eq!(
        std::fs::read(path.with_extension("bin")).unwrap(),
        result.stream
    );
    result.identity.model = "DR/8 x".into();
    let odd = save_backup(&result, &dir, "before_write", "20261008_180001").unwrap();
    assert!(odd.ends_with("20261008_180001_before_write_DR_8_x.F136"));
    result.identity.model.clear();
    let unknown = save_backup(&result, &dir, "read", "20261008_180002").unwrap();
    assert!(unknown.ends_with("20261008_180002_read_radio.F136"));
    let _ = std::fs::remove_dir_all(&dir);
}
