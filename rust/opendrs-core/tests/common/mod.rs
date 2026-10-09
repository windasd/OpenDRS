//! Shared helpers for the integration tests: fixture paths and the capture replay transport.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::PathBuf;
use std::time::Duration;

use opendrs_core::radio_link::{RadioError, Transport, bytes_repr};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

pub fn cap(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("capture").join(name)).expect("capture fixture")
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
