//! A simulated radio answering the recording commands of [`crate::recording`] (port of
//! `recording_sim.py`).
//!
//! Follows the firmware's handlers (record line, then "Record Read"; 4050 bytes per "Read";
//! "ChecksumR" + sum right after the last block) and the list frame as Wav.exe expects it (the
//! 0x58 frame's payload is a record number, as in the firmware's handler). Either command can be
//! switched off to imitate a radio whose firmware does not route it.

use std::collections::VecDeque;
use std::time::Duration;

use opendrs_core::radio_link::{RadioError, Transport, byte_sum};

use crate::recording::{
    BLOCK, CHECKSUM_READ, LIST_CMD, READ_REQUEST, RECORD_COMMAND_SIZE, RECORD_VERB, RecordInfo,
    checksum16,
};

/// Three recordings: received, sent, and one of exactly one block.
pub fn demo_records() -> Vec<(RecordInfo, Vec<u8>)> {
    let specs = [
        ('R', 9_998_765, 8433_usize, "2025/06/07 12:34"),
        ('S', 12345, 3000, "2025/06/08 08:05"),
        ('R', 9, 4050, "2025/06/09 23:59"),
    ];
    let total = specs.len() as u64;
    specs
        .iter()
        .zip(1_usize..)
        .map(|(&(direction, call_id, length, time), n)| {
            let data = (0..length)
                .map(|i| ((i * 31 + n * 7 + (i >> 8)) & 0xFF) as u8)
                .collect();
            let info = RecordInfo {
                number: n as u64,
                total,
                direction,
                call_id,
                length: length as u64,
                time: time.to_owned(),
            };
            (info, data)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimulatedRecorder {
    pub records: Vec<(RecordInfo, Vec<u8>)>,
    pub supports_list: bool,
    pub supports_record_read: bool,
    pub corrupt_checksum: bool,
    pub sent: Vec<u8>,
    input: Vec<u8>,
    output: Vec<u8>,
    reading: Option<VecDeque<Vec<u8>>>,
    sum: u32,
}

impl Default for SimulatedRecorder {
    fn default() -> Self {
        SimulatedRecorder::with_records(demo_records())
    }
}

fn be_u32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |acc, &b| (acc << 8) | u32::from(b))
}

impl SimulatedRecorder {
    pub fn with_records(records: Vec<(RecordInfo, Vec<u8>)>) -> Self {
        SimulatedRecorder {
            records,
            supports_list: true,
            supports_record_read: true,
            corrupt_checksum: false,
            sent: Vec::new(),
            input: Vec::new(),
            output: Vec::new(),
            reading: None,
            sum: 0,
        }
    }

    /// The stored record with this number (1-based).
    fn record(&self, number: u32) -> Option<&(RecordInfo, Vec<u8>)> {
        let index = usize::try_from(number).ok()?.checked_sub(1)?;
        self.records.get(index)
    }

    /// Handles one complete command at the start of the input; false when more bytes are needed.
    fn step(&mut self) -> bool {
        let buf = &self.input;
        if buf.starts_with(RECORD_VERB) && buf.len() >= RECORD_COMMAND_SIZE {
            let number = be_u32(&buf[11..15]);
            self.input.drain(..RECORD_COMMAND_SIZE);
            if self.supports_record_read {
                self.start_read(number);
            }
            return true;
        }
        if buf.starts_with(READ_REQUEST) && buf.len() >= READ_REQUEST.len() {
            self.input.drain(..READ_REQUEST.len());
            self.next_block();
            return true;
        }
        if buf.first() == Some(&0x68) && buf.len() >= 8 && buf.len() >= 9 + usize::from(buf[6]) {
            let size = 9 + usize::from(buf[6]);
            let frame: Vec<u8> = self.input.drain(..size).collect();
            self.list_frame(&frame);
            return true;
        }
        let waiting = RECORD_VERB.starts_with(&buf[..buf.len().min(11)])
            || READ_REQUEST.starts_with(&buf[..buf.len().min(4)])
            || buf.first() == Some(&0x68);
        if !buf.is_empty() && !waiting {
            self.input.clear(); // noise
        }
        false
    }

    fn list_frame(&mut self, frame: &[u8]) {
        let blanked = [&frame[..4], &[0, 0], &frame[6..]].concat();
        let ok = u32::from(checksum16(&blanked)) == be_u32(&frame[4..6]);
        // firmware 0x030272c4: the payload is the record number
        let number = be_u32(&frame[8..frame.len().min(10)]);
        if !(self.supports_list && ok && frame[1] == LIST_CMD) {
            return;
        }
        if let Some((info, _)) = self.record(number) {
            let line = info.line();
            self.output.extend(line);
        }
    }

    fn start_read(&mut self, number: u32) {
        self.sum = 0;
        let Some((info, data)) = self.record(number) else {
            self.output.extend_from_slice(RECORD_VERB);
            // the firmware still answers one stale block
            self.reading = Some(VecDeque::from([vec![0; BLOCK]]));
            return;
        };
        let size = usize::try_from(info.blocks())
            .unwrap_or(usize::MAX)
            .saturating_mul(BLOCK);
        let mut padded = data.clone();
        if padded.len() < size {
            padded.resize(size, 0xFF);
        }
        let blocks = padded[..size].chunks(BLOCK).map(<[u8]>::to_vec).collect();
        let line = info.line();
        self.output.extend(line);
        self.output.extend_from_slice(RECORD_VERB);
        self.reading = Some(blocks);
    }

    fn next_block(&mut self) {
        let Some(block) = self.reading.as_mut().and_then(VecDeque::pop_front) else {
            return;
        };
        self.sum = self.sum.wrapping_add(byte_sum(&block));
        self.output.extend(block);
        if self.reading.as_ref().is_some_and(VecDeque::is_empty) {
            let total = self.sum.wrapping_add(u32::from(self.corrupt_checksum));
            self.output.extend_from_slice(CHECKSUM_READ);
            self.output.extend_from_slice(&total.to_be_bytes());
            self.reading = None;
        }
    }
}

impl Transport for SimulatedRecorder {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.sent.extend_from_slice(data);
        self.input.extend_from_slice(data);
        while self.step() {}
        Ok(())
    }

    /// Like the Python simulator: hands out what is there, possibly fewer than `n` bytes.
    fn read_exact(&mut self, n: usize, _timeout: Duration) -> Result<Vec<u8>, RadioError> {
        let n = n.min(self.output.len());
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
