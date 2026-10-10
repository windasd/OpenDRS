//! Shared helpers for the integration tests: fixture paths and the capture replay transport.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::PathBuf;
use std::time::Duration;

use opendrs_core::codeplug::BLOCK;
use opendrs_core::radio_link::{RadioError, Transport, bytes_repr};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

pub fn cap(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("capture").join(name)).expect("capture fixture")
}

/// The captured read: (what the PC sent, what the radio answered) per step.
pub fn read_exchanges() -> Vec<(Vec<u8>, Vec<u8>)> {
    let stream = cap("read_stream.bin");
    let mut ex = vec![(cap("read_cmd.bin"), cap("read_reply.bin"))];
    ex.extend(stream.chunks(BLOCK).map(|b| (b"Read".to_vec(), b.to_vec())));
    if let Some(last) = ex.last_mut() {
        last.1.extend(cap("read_checksum.bin"));
    }
    ex
}

/// The captured write: (what the PC sent, what the radio answered) per step.
pub fn write_exchanges() -> Vec<(Vec<u8>, Vec<u8>)> {
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

/// A line where nothing ever answers.
pub struct Silent;

impl Transport for Silent {
    fn write(&mut self, _data: &[u8]) -> Result<(), RadioError> {
        Ok(())
    }
    fn read_exact(&mut self, n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        Err(RadioError::Timeout {
            message: format!("等待對講機回應逾時（預期 {n} bytes，收到 0）"),
            received: 0,
        })
    }
    fn read_idle(&mut self, _idle: Duration, _max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        Ok(Vec::new())
    }
    fn reset_input(&mut self) -> Result<(), RadioError> {
        Ok(())
    }
}

/// Plays back the radio side of the capture and checks every byte the PC sends.
pub struct ReplayTransport {
    exchanges: Vec<(Vec<u8>, Vec<u8>)>,
    pending: Vec<u8>,
    output: Vec<u8>,
    pub sent: Vec<u8>,
}

impl ReplayTransport {
    pub fn new(exchanges: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        ReplayTransport {
            exchanges,
            pending: Vec::new(),
            output: Vec::new(),
            sent: Vec::new(),
        }
    }

    pub fn finished(&self) -> bool {
        self.exchanges.is_empty() && self.pending.is_empty()
    }
}

impl Transport for ReplayTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.sent.extend_from_slice(data);
        self.pending.extend_from_slice(data);
        while let Some((expected, _)) = self.exchanges.first() {
            if self.pending.len() < expected.len() {
                break;
            }
            let (expected, response) = self.exchanges.remove(0);
            let got: Vec<u8> = self.pending.drain(..expected.len()).collect();
            assert_eq!(
                got,
                expected,
                "PC sent {}, capture has {}",
                bytes_repr(&got[..got.len().min(32)]),
                bytes_repr(&expected[..expected.len().min(32)])
            );
            self.output.extend(response);
        }
        Ok(())
    }

    fn read_exact(&mut self, n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        if self.output.len() < n {
            return Err(RadioError::Timeout {
                message: "replay exhausted".into(),
                received: self.output.len(),
            });
        }
        Ok(self.output.drain(..n).collect())
    }

    fn read_idle(&mut self, _idle: Duration, _max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        Ok(std::mem::take(&mut self.output))
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.output.clear();
        Ok(())
    }
}
