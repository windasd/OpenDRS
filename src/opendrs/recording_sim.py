"""A simulated radio answering the recording commands of :mod:`opendrs.recording`.

Follows the firmware's handlers (record line, then "Record Read"; 4050 bytes per "Read";
"ChecksumR" + sum right after the last block) and the list frame as Wav.exe expects it
(the 0x58 frame's payload is a record number, as in the firmware's handler). Either command can be switched off to imitate
a radio whose firmware does not route it.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .radio_link import READ_REQUEST
from .recording import (
    BLOCK,
    CHECKSUM_READ,
    LIST_CMD,
    RECORD_VERB,
    RecordInfo,
    checksum16,
)


def demo_records() -> list[tuple[RecordInfo, bytes]]:
    specs = [("R", 9998765, 8433, "2025/06/07 12:34"), ("S", 12345, 3000, "2025/06/08 08:05"),
             ("R", 9, 4050, "2025/06/09 23:59")]
    out = []
    for n, (direction, call_id, length, when) in enumerate(specs, start=1):
        data = bytes((i * 31 + n * 7 + (i >> 8)) & 0xFF for i in range(length))
        out.append((RecordInfo(n, len(specs), direction, call_id, length, when), data))
    return out


@dataclass
class SimulatedRecorder:
    records: list[tuple[RecordInfo, bytes]] = field(default_factory=demo_records)
    supports_list: bool = True
    supports_record_read: bool = True
    corrupt_checksum: bool = False
    sent: bytearray = field(default_factory=bytearray)
    _in: bytearray = field(default_factory=bytearray)
    _out: bytearray = field(default_factory=bytearray)
    _reading: list[bytes] | None = None
    _sum: int = 0

    def write(self, data: bytes) -> None:
        self.sent += data
        self._in += data
        while self._step():
            pass

    def _step(self) -> bool:
        buf = bytes(self._in)
        if buf.startswith(RECORD_VERB) and len(buf) >= 19:
            del self._in[:19]
            if self.supports_record_read:
                self._start_read(int.from_bytes(buf[11:15], "big"))
            return True
        if buf.startswith(READ_REQUEST) and len(buf) >= 4:
            del self._in[:4]
            self._next_block()
            return True
        if buf[:1] == b"\x68" and len(buf) >= 8 and len(buf) >= 9 + buf[6]:
            frame = buf[:9 + buf[6]]
            del self._in[:len(frame)]
            ok = checksum16(frame[:4] + b"\0\0" + frame[6:]) == int.from_bytes(frame[4:6], "big")
            number = int.from_bytes(frame[8:10], "big")  # firmware 0x030272c4: the record number
            if self.supports_list and ok and frame[1] == LIST_CMD and 1 <= number <= len(self.records):
                self._out += self.records[number - 1][0].line()
            return True
        if buf and not (RECORD_VERB.startswith(buf[:11]) or READ_REQUEST.startswith(buf[:4]) or buf[:1] == b"\x68"):
            self._in.clear()  # noise
        return False

    def _start_read(self, number: int) -> None:
        if not 1 <= number <= len(self.records):
            self._out += RECORD_VERB
            self._reading = [bytes(BLOCK)]  # the firmware still answers one stale block
            self._sum = 0
            return
        info, data = self.records[number - 1]
        self._out += info.line() + RECORD_VERB
        padded = data + b"\xff" * (info.blocks * BLOCK - len(data))
        self._reading = [padded[i * BLOCK:(i + 1) * BLOCK] for i in range(info.blocks)]
        self._sum = 0

    def _next_block(self) -> None:
        if not self._reading:
            return
        block = self._reading.pop(0)
        self._out += block
        self._sum += sum(block)
        if not self._reading:
            total = (self._sum + (1 if self.corrupt_checksum else 0)) & 0xFFFFFFFF
            self._out += CHECKSUM_READ + total.to_bytes(4, "big")
            self._reading = None

    # -- Transport interface -------------------------------------------------
    def read_idle(self, idle: float, max_wait: float) -> bytes:
        data = bytes(self._out)
        self._out.clear()
        return data

    def read_exact(self, n: int, timeout: float) -> bytes:
        data = bytes(self._out[:n])
        del self._out[:n]
        return data

    def reset_input(self) -> None:
        self._out.clear()

    def close(self) -> None:
        pass
