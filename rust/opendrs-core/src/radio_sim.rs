//! A simulated radio implementing the radio side of the programming protocol (port of
//! `radio_sim.py`), modelled on the firmware handlers: counts read from the settings area,
//! part-1 block count re-derived from the third block during a write, staging then commit.

use std::time::Duration;

use crate::codeplug::{BLOCK, COUNTS_OFFSET, Counts, PART1_ADDR};
use crate::radio_link::{
    CHECKSUM_READ, CHECKSUM_WRITE, COMMAND_SIZE, READ_PREFIX, READ_REQUEST, READ_VERB, RadioError,
    Transport, WRITE_ACK, WRITE_PREFIX, WRITE_VERB, byte_sum,
};

pub const AREA: usize = 0x40000;

pub fn default_identity() -> Vec<u8> {
    [
        &b"308E.BBKTARSCB.003.Mar  9 2024"[..],
        &[0; 7],
        b"17:02:38",
        &[0; 2],
        b"+DRS-8500+136-480MHZ+",
        &[b'y'; 12],
    ]
    .concat()
}

#[derive(Clone, Debug)]
enum Mode {
    Idle,
    Read {
        total: usize,
        sent: usize,
        part1_blocks: usize,
        sum: u32,
    },
    Write {
        total: usize,
        blocks: Vec<Vec<u8>>,
        sum: u32,
    },
}

#[derive(Clone, Debug)]
pub struct SimulatedRadio {
    pub part1: Vec<u8>,
    pub part2: Vec<u8>,
    pub identity: Vec<u8>,
    /// Fault injection: report a write checksum that is off by one.
    pub corrupt_write_checksum: bool,
    /// Fault injection: stop acknowledging blocks after this many.
    pub stop_acking_after: Option<usize>,
    pub commits: usize,
    input: Vec<u8>,
    output: Vec<u8>,
    mode: Mode,
}

impl Default for SimulatedRadio {
    fn default() -> Self {
        SimulatedRadio {
            part1: vec![0; AREA],
            part2: vec![0; AREA],
            identity: default_identity(),
            corrupt_write_checksum: false,
            stop_acking_after: None,
            commits: 0,
            input: Vec::new(),
            output: Vec::new(),
            mode: Mode::Idle,
        }
    }
}

impl SimulatedRadio {
    pub fn with_stream(stream: &[u8], counts: Counts) -> Self {
        let mut radio = SimulatedRadio::default();
        let p1 = counts.part1_blocks() * BLOCK;
        radio.part1[..p1].copy_from_slice(&stream[..p1]);
        radio.part2[..stream.len() - p1].copy_from_slice(&stream[p1..]);
        radio
    }

    pub fn counts(&self) -> Counts {
        let at = |i: usize| {
            usize::from(u16::from_le_bytes([
                self.part1[COUNTS_OFFSET + i],
                self.part1[COUNTS_OFFSET + i + 1],
            ]))
        };
        Counts::new(at(0), at(2), at(4))
    }

    fn process(&mut self) {
        loop {
            match &mut self.mode {
                Mode::Idle => {
                    if self.input.len() < COMMAND_SIZE {
                        return;
                    }
                    let cmd: Vec<u8> = self.input.drain(..COMMAND_SIZE).collect();
                    let addr = u32::from_be_bytes([cmd[11], cmd[12], cmd[13], cmd[14]]);
                    let blocks = u32::from_be_bytes([cmd[15], cmd[16], cmd[17], cmd[18]]) as usize;
                    if addr != PART1_ADDR {
                        continue;
                    }
                    if cmd[..11] == READ_VERB[..] {
                        self.start_read();
                    } else if cmd[..11] == WRITE_VERB[..] {
                        self.output.extend_from_slice(WRITE_PREFIX);
                        self.output.extend_from_slice(&self.identity);
                        self.mode = Mode::Write {
                            total: blocks,
                            blocks: Vec::new(),
                            sum: 0,
                        };
                    }
                }
                Mode::Read { .. } => {
                    if self.input.len() < READ_REQUEST.len() {
                        return;
                    }
                    let request: Vec<u8> = self.input.drain(..READ_REQUEST.len()).collect();
                    if request == READ_REQUEST {
                        self.send_block();
                    }
                }
                Mode::Write { .. } => {
                    if self.input.len() < BLOCK {
                        return;
                    }
                    let block: Vec<u8> = self.input.drain(..BLOCK).collect();
                    self.receive_block(block);
                }
            }
        }
    }

    fn start_read(&mut self) {
        let mut c = self.counts();
        if c.contacts > 1000 {
            c = Counts::new(c.zones, c.channels, 0); // firmware: count >= 0x3E9 -> treated as empty
        }
        self.output.extend_from_slice(READ_PREFIX);
        for v in [
            c.zones,
            c.channels,
            c.contacts,
            c.part1_blocks(),
            c.part2_blocks(),
        ] {
            self.output
                .extend_from_slice(&u16::try_from(v).unwrap_or(u16::MAX).to_le_bytes());
        }
        self.output.extend_from_slice(&self.identity);
        self.mode = Mode::Read {
            total: c.total_blocks(),
            sent: 0,
            part1_blocks: c.part1_blocks(),
            sum: 0,
        };
    }

    fn send_block(&mut self) {
        let Mode::Read {
            total,
            sent,
            part1_blocks,
            sum,
        } = &mut self.mode
        else {
            return;
        };
        let i = *sent;
        let (area, offset) = if i < *part1_blocks {
            (&self.part1, i * BLOCK)
        } else {
            (&self.part2, (i - *part1_blocks) * BLOCK)
        };
        let block = &area[offset..offset + BLOCK];
        *sum = sum.wrapping_add(byte_sum(block));
        self.output.extend_from_slice(block);
        *sent += 1;
        if *sent >= *total {
            let tail = [CHECKSUM_READ, &sum.to_be_bytes()].concat();
            self.output.extend(tail);
            self.mode = Mode::Idle;
        }
    }

    fn receive_block(&mut self, block: Vec<u8>) {
        let Mode::Write { total, blocks, sum } = &mut self.mode else {
            return;
        };
        *sum = sum.wrapping_add(byte_sum(&block));
        blocks.push(block);
        let n = blocks.len();
        if n < *total {
            if self.stop_acking_after.is_none_or(|limit| n <= limit) {
                self.output.extend_from_slice(WRITE_ACK);
            }
            return;
        }
        let reported = sum.wrapping_add(u32::from(self.corrupt_write_checksum));
        let blocks = std::mem::take(blocks);
        self.output.extend_from_slice(CHECKSUM_WRITE);
        self.output.extend_from_slice(&reported.to_be_bytes());
        self.output
            .extend_from_slice(b"id 98eb,Manufacture:TC58\r\n");
        self.commit(&blocks);
        self.mode = Mode::Idle;
    }

    fn commit(&mut self, blocks: &[Vec<u8>]) {
        // part-1 length comes from the contact count in the third block (offset 0x393)
        let at = COUNTS_OFFSET + 4 - 2 * BLOCK;
        let contacts = blocks
            .get(2)
            .map_or(0, |b| usize::from(u16::from_le_bytes([b[at], b[at + 1]])));
        let p1 = ((contacts * 16 + 0x1BFF) >> 11) * BLOCK;
        let data = blocks.concat();
        let p1 = p1.min(data.len());
        self.part1[..p1].copy_from_slice(&data[..p1]);
        self.part2[..data.len() - p1].copy_from_slice(&data[p1..]);
        self.commits += 1;
    }
}

impl Transport for SimulatedRadio {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.input.extend_from_slice(data);
        self.process();
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
        Ok(std::mem::take(&mut self.output))
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.output.clear();
        Ok(())
    }
}
