"""A simulated IAP bootloader (the radio side of :mod:`opendrs.iap`) for tests and dry runs.

Behaves like the receiving side the vendor tool and the radio's clone code expect:
"Erase" + address + last page index -> erase reply, each 2048-byte page -> "kyd",
after the last page -> "ChecksumI" + u32 BE byte sum.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .iap import CHECKSUM_TAG, ERASE_VERB, PAGE_ACK, PAGE_SIZE
from .radio_link import RadioTimeout

ERASE_COMMAND_SIZE = len(ERASE_VERB) + 8
DEFAULT_ERASE_REPLY = b"IAP Erase ok"


@dataclass
class SimulatedBootloader:
    erase_reply: bytes = DEFAULT_ERASE_REPLY
    ack_suffix: bytes = b""  # extra text after each "kyd" (e.g. b"\r\n" or debug output)
    # fault injection
    silent: bool = False
    stop_acking_after: int | None = None
    bad_ack: bytes | None = None
    corrupt_checksum: bool = False
    # what was received
    address: int | None = None
    last_page: int | None = None
    flash: bytearray = field(default_factory=bytearray)
    erased: bool = False
    finished: bool = False
    sent: bytearray = field(default_factory=bytearray)
    _in: bytearray = field(default_factory=bytearray)
    _out: bytearray = field(default_factory=bytearray)
    _pages: int = 0

    def write(self, data: bytes) -> None:
        self.sent += data
        if self.silent:
            return
        self._in += data
        if self.last_page is None:
            if len(self._in) < ERASE_COMMAND_SIZE:
                return
            cmd = bytes(self._in[:ERASE_COMMAND_SIZE])
            del self._in[:ERASE_COMMAND_SIZE]
            if not cmd.startswith(ERASE_VERB):
                return
            self.address = int.from_bytes(cmd[11:15], "big")
            self.last_page = int.from_bytes(cmd[15:19], "big")
            self.erased = True
            self._out += self.erase_reply
        while len(self._in) >= PAGE_SIZE and not self.finished:
            self.flash += self._in[:PAGE_SIZE]
            del self._in[:PAGE_SIZE]
            self._pages += 1
            if self.stop_acking_after is not None and self._pages > self.stop_acking_after:
                continue
            self._out += (self.bad_ack or PAGE_ACK) + self.ack_suffix
            if self._pages == self.last_page + 1:
                total = (sum(self.flash) + (1 if self.corrupt_checksum else 0)) & 0xFFFFFFFF
                self._out += CHECKSUM_TAG + total.to_bytes(4, "big")
                self.finished = True

    def read_exact(self, n: int, timeout: float) -> bytes:
        if len(self._out) < n:
            got = len(self._out)
            self._out.clear()
            raise RadioTimeout(f"等待對講機回應逾時（預期 {n} bytes，收到 {got}）", got)
        data = bytes(self._out[:n])
        del self._out[:n]
        return data

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        if not self._out:
            raise RadioTimeout("模擬 bootloader 沒有回應", 0)
        data = bytes(self._out)
        self._out.clear()
        return data

    def reset_input(self) -> None:
        self._out.clear()

    def close(self) -> None:
        pass
