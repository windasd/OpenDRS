"""A simulated radio implementing the radio side of the programming protocol.

Modelled on the firmware handlers (FUN_030271ac, FUN_030272fc, FUN_03027488,
FUN_03027670, FUN_03027cd4): counts read from the settings area, part-1 block
count re-derived from the third block during a write, staging then commit.
Used by the tests and by the "simulated radio" option in the GUI.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

from .codeplug import BLOCK, COUNTS_OFFSET, PART1_ADDR, Counts
from .radio_link import (
    CHECKSUM_READ,
    CHECKSUM_WRITE,
    READ_PREFIX,
    READ_REQUEST,
    READ_VERB,
    WRITE_ACK,
    WRITE_PREFIX,
    WRITE_VERB,
    RadioTimeout,
)

AREA = 0x40000
DEFAULT_IDENTITY = (b"308E.BBKTARSCB.003.Mar  9 2024" + bytes(7) + b"17:02:38" + bytes(2)
                    + b"+DRS-8500+136-480MHZ+" + b"y" * 12)


@dataclass
class SimulatedRadio:
    part1: bytearray = field(default_factory=lambda: bytearray(AREA))
    part2: bytearray = field(default_factory=lambda: bytearray(AREA))
    identity: bytes = DEFAULT_IDENTITY
    # fault injection
    corrupt_write_checksum: bool = False
    stop_acking_after: int | None = None
    commits: int = 0
    _in: bytearray = field(default_factory=bytearray)
    _out: bytearray = field(default_factory=bytearray)
    _mode: str = "idle"
    _blocks: list[bytes] = field(default_factory=list)
    _total: int = 0
    _sent: int = 0
    _sum: int = 0

    @classmethod
    def with_stream(cls, stream: bytes, counts: Counts) -> SimulatedRadio:
        radio = cls()
        p1 = counts.part1_blocks * BLOCK
        radio.part1[:p1] = stream[:p1]
        radio.part2[:len(stream) - p1] = stream[p1:]
        return radio

    # -- Transport interface -------------------------------------------------
    def write(self, data: bytes) -> None:
        self._in += data
        self._process()

    def read_exact(self, n: int, timeout: float) -> bytes:
        if len(self._out) < n:
            got = len(self._out)
            self._out.clear()
            raise RadioTimeout(f"等待對講機回應逾時（預期 {n} bytes，收到 {got}）", got)
        data = bytes(self._out[:n])
        del self._out[:n]
        return data

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        data = bytes(self._out)
        self._out.clear()
        return data

    def reset_input(self) -> None:
        self._out.clear()

    def close(self) -> None:
        pass

    # -- radio side ----------------------------------------------------------
    def counts(self) -> Counts:
        return Counts(*struct.unpack_from("<3H", self.part1, COUNTS_OFFSET))

    def _process(self) -> None:
        while True:
            if self._mode == "idle":
                if len(self._in) < 31:
                    return
                cmd = bytes(self._in[:31])
                del self._in[:31]
                verb, (addr, blocks) = cmd[:11], struct.unpack_from(">II", cmd, 11)
                if addr != PART1_ADDR or verb not in (READ_VERB, WRITE_VERB):
                    continue
                if verb == READ_VERB:
                    self._start_read()
                else:
                    self._out += WRITE_PREFIX + self.identity
                    self._mode, self._total, self._blocks, self._sum = "write", blocks, [], 0
            elif self._mode == "read":
                if len(self._in) < 4:
                    return
                req = bytes(self._in[:4])
                del self._in[:4]
                if req == READ_REQUEST:
                    self._send_block()
            elif self._mode == "write":
                if len(self._in) < BLOCK:
                    return
                block = bytes(self._in[:BLOCK])
                del self._in[:BLOCK]
                self._receive_block(block)
            else:  # pragma: no cover
                return

    def _start_read(self) -> None:
        c = self.counts()
        if c.contacts > 1000:  # firmware: count >= 0x3E9 -> treated as empty
            c = Counts(c.zones, c.channels, 0)
        self._out += READ_PREFIX + struct.pack("<5H", c.zones, c.channels, c.contacts,
                                               c.part1_blocks, c.part2_blocks) + self.identity
        self._mode, self._total, self._sent, self._sum = "read", c.total_blocks, 0, 0
        self._p1 = c.part1_blocks

    def _send_block(self) -> None:
        i = self._sent
        area, off = (self.part1, i * BLOCK) if i < self._p1 else (self.part2, (i - self._p1) * BLOCK)
        block = bytes(area[off:off + BLOCK])
        self._sum += sum(block)
        self._out += block
        self._sent += 1
        if self._sent >= self._total:
            self._out += CHECKSUM_READ + struct.pack(">I", self._sum & 0xFFFFFFFF)
            self._mode = "idle"

    def _receive_block(self, block: bytes) -> None:
        self._blocks.append(block)
        self._sum += sum(block)
        n = len(self._blocks)
        if n < self._total:
            if self.stop_acking_after is None or n <= self.stop_acking_after:
                self._out += WRITE_ACK
            return
        total = (self._sum + (1 if self.corrupt_write_checksum else 0)) & 0xFFFFFFFF
        self._out += CHECKSUM_WRITE + struct.pack(">I", total) + b"id 98eb,Manufacture:TC58\r\n"
        self._commit()
        self._mode = "idle"

    def _commit(self) -> None:
        # part-1 length comes from the contact count in the third block (offset 0x393)
        n = struct.unpack_from("<H", self._blocks[2], COUNTS_OFFSET + 4 - 2 * BLOCK)[0]
        p1 = (n * 16 + 0x1BFF) >> 11
        data = b"".join(self._blocks)
        self.part1[:p1 * BLOCK] = data[:p1 * BLOCK]
        self.part2[:len(data) - p1 * BLOCK] = data[p1 * BLOCK:]
        self.commits += 1
