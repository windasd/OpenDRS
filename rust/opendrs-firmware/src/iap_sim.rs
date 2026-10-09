//! A simulated IAP bootloader, the radio side of [`crate::iap`], for tests and dry runs (port of
//! `iap_sim.py`).
//!
//! Behaves like the receiving side the vendor tool and the radio's clone code expect:
//! "Erase" + address + last page index -> erase reply, each 2048-byte page -> "kyd",
//! after the last page -> "ChecksumI" + u32 BE byte sum.

use std::time::Duration;

use opendrs_core::radio_link::{RadioError, Transport, byte_sum};

use crate::iap::{CHECKSUM_TAG, ERASE_COMMAND_SIZE, ERASE_VERB, PAGE_ACK, PAGE_SIZE};

pub const DEFAULT_ERASE_REPLY: &[u8] = b"IAP Erase ok";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulatedBootloader {
    pub erase_reply: Vec<u8>,
    /// Extra text after each "kyd" (e.g. "\r\n" or debug output).
    pub ack_suffix: Vec<u8>,
    // fault injection
    pub silent: bool,
    pub stop_acking_after: Option<usize>,
    /// Sent instead of "kyd" (an empty value means "kyd", as in Python).
    pub bad_ack: Option<Vec<u8>>,
    pub corrupt_checksum: bool,
    // what was received
    pub address: Option<u32>,
    pub last_page: Option<u32>,
    pub flash: Vec<u8>,
    pub erased: bool,
    pub finished: bool,
    pub sent: Vec<u8>,
    input: Vec<u8>,
    output: Vec<u8>,
    pages: usize,
}

impl Default for SimulatedBootloader {
    fn default() -> Self {
        SimulatedBootloader {
            erase_reply: DEFAULT_ERASE_REPLY.to_vec(),
            ack_suffix: Vec::new(),
            silent: false,
            stop_acking_after: None,
            bad_ack: None,
            corrupt_checksum: false,
            address: None,
            last_page: None,
            flash: Vec::new(),
            erased: false,
            finished: false,
            sent: Vec::new(),
            input: Vec::new(),
            output: Vec::new(),
            pages: 0,
        }
    }
}

fn be_u32(bytes: &[u8]) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[..4]);
    u32::from_be_bytes(word)
}

impl SimulatedBootloader {
    /// Takes the erase command once enough bytes are in; false while waiting for it (or when
    /// it was not an erase command).
    fn take_erase_command(&mut self) -> bool {
        if self.last_page.is_some() {
            return true;
        }
        if self.input.len() < ERASE_COMMAND_SIZE {
            return false;
        }
        let command: Vec<u8> = self.input.drain(..ERASE_COMMAND_SIZE).collect();
        if !command.starts_with(ERASE_VERB) {
            return false;
        }
        self.address = Some(be_u32(&command[11..15]));
        self.last_page = Some(be_u32(&command[15..19]));
        self.erased = true;
        self.output.extend_from_slice(&self.erase_reply);
        true
    }

    fn take_pages(&mut self) {
        while self.input.len() >= PAGE_SIZE && !self.finished {
            let page: Vec<u8> = self.input.drain(..PAGE_SIZE).collect();
            self.flash.extend(page);
            self.pages += 1;
            if self.stop_acking_after.is_some_and(|n| self.pages > n) {
                continue;
            }
            let ack = match &self.bad_ack {
                Some(ack) if !ack.is_empty() => ack.clone(),
                _ => PAGE_ACK.to_vec(),
            };
            self.output.extend(ack);
            self.output.extend_from_slice(&self.ack_suffix);
            let last = self.last_page.map(u64::from);
            if last.is_some_and(|last| self.pages as u64 == last + 1) {
                let total = byte_sum(&self.flash).wrapping_add(u32::from(self.corrupt_checksum));
                self.output.extend_from_slice(CHECKSUM_TAG);
                self.output.extend_from_slice(&total.to_be_bytes());
                self.finished = true;
            }
        }
    }
}

impl Transport for SimulatedBootloader {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.sent.extend_from_slice(data);
        if self.silent {
            return Ok(());
        }
        self.input.extend_from_slice(data);
        if self.take_erase_command() {
            self.take_pages();
        }
        Ok(())
    }

    fn read_exact(&mut self, n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        if self.output.len() < n {
            let got = self.output.len();
            self.output.clear();
            return Err(RadioError::Timeout {
                message: format!("等待對講機回應逾時（預期 {n} bytes，收到 {got}）"),
                received: got,
            });
        }
        Ok(self.output.drain(..n).collect())
    }

    fn read_idle(&mut self, _idle: Duration, _max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        if self.output.is_empty() {
            return Err(RadioError::Timeout {
                message: "模擬 bootloader 沒有回應".into(),
                received: 0,
            });
        }
        Ok(std::mem::take(&mut self.output))
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.output.clear();
        Ok(())
    }
}
