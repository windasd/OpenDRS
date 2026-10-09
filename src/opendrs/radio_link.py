"""Read / write a radio over the programming cable.

Protocol (from a USB capture of the legacy CPS, cross-checked with the firmware):

    PC  -> "Flash Read " | "Flash Write"   11-byte verb
           + start address  u32 BE (0x003C0000)
           + block count    u32 BE (write: must equal part1 + part2)
           + write password 6 bytes, read password 6 bytes   (31 bytes total)
    RF  -> "  Read_2M_\\0\\0" + zones, channels, contacts, part1, part2 (u16 LE) + identity
           "  Write_2M_\\0" + identity
    read:  PC "Read" -> RF 2048 bytes, per block; then RF "ChecksumR" + u32 BE sum
    write: PC 2048 bytes -> RF "Write" (not after the last block); then RF
           "ChecksumW" + u32 BE sum, followed by free-form debug text.

The radio stages a write at flash 0x600000 and only copies it over the live
codeplug once every block has arrived, so an interrupted write leaves the radio as
it was. The checksum is a plain byte sum: it detects corruption but by the time
it arrives the radio has already committed — so callers should always keep a
backup read taken just before writing.
"""

from __future__ import annotations

import re
import struct
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Protocol

from .codeplug import (
    BLOCK,
    PART1_ADDR,
    CodeplugError,
    Counts,
    build_stream,
    parse_stream,
)
from .f136 import Project

BAUD = 115_200
READ_VERB = b"Flash Read "
WRITE_VERB = b"Flash Write"
READ_PREFIX = b"  Read_2M_\x00\x00"
WRITE_PREFIX = b"  Write_2M_\x00"
READ_REQUEST = b"Read"
WRITE_ACK = b"Write"
CHECKSUM_READ = b"ChecksumR"
CHECKSUM_WRITE = b"ChecksumW"
# The legacy CPS always sent 6 here for reads; the radio recomputes the count itself.
LEGACY_READ_BLOCKS = 6
PASSWORD_SIZE = 6

Progress = Callable[[int, int, str], None]


class RadioError(RuntimeError):
    """Communication failure, with a message suitable for the user."""


class RadioTimeout(RadioError):
    def __init__(self, message: str, received: int = -1) -> None:
        super().__init__(message)
        self.received = received  # bytes that did arrive (-1 = unknown)


NO_REPLY = ("對講機沒有任何回應。請確認：\n"
            "1. 對講機已開機，停在待機畫面（不在選單、掃描或通話中）；\n"
            "2. 寫頻線兩端都插緊（尤其對講機側的插頭）；\n"
            "3. 連接埠選的是寫頻線（目前的 COM 埠）；\n"
            "4. 仍不行時將對講機關機再開機後重試。")


class Transport(Protocol):
    def write(self, data: bytes) -> None: ...
    def read_exact(self, n: int, timeout: float) -> bytes: ...
    def read_idle(self, idle: float, max_wait: float) -> bytes: ...
    def reset_input(self) -> None: ...
    def close(self) -> None: ...


# ------------------------------------------------------------------ serial port


@dataclass(frozen=True)
class PortInfo:
    device: str
    description: str
    is_programming_cable: bool


KNOWN_CABLE_VIDS = {0x067B, 0x1A86, 0x10C4, 0x0403}  # Prolific, WCH, Silabs, FTDI


def list_ports() -> list[PortInfo]:
    from serial.tools import list_ports as lp

    ports = [PortInfo(p.device, p.description or p.device, (p.vid or 0) in KNOWN_CABLE_VIDS)
             for p in lp.comports()]
    return sorted(ports, key=lambda p: (not p.is_programming_cable, p.device))


class SerialTransport:
    def __init__(self, port: str, baud: int = BAUD) -> None:
        import serial

        try:
            # serial_for_url also accepts "loop://" (used by the tests)
            self._s = serial.serial_for_url(port, baud, bytesize=8, parity="N", stopbits=1,
                                            timeout=0.05, write_timeout=5)
        except (serial.SerialException, ValueError) as exc:
            raise RadioError(f"無法開啟 {port}：{exc}") from exc
        self._s.dtr = True
        self._s.rts = True

    def write(self, data: bytes) -> None:
        self._s.write(data)
        self._s.flush()

    def read_exact(self, n: int, timeout: float) -> bytes:
        buf = bytearray()
        deadline = time.monotonic() + timeout
        while len(buf) < n:
            if time.monotonic() > deadline:
                raise RadioTimeout(f"等待對講機回應逾時（預期 {n} bytes，收到 {len(buf)}）", len(buf))
            buf += self._s.read(n - len(buf))
        return bytes(buf)

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        buf = bytearray()
        start = last = time.monotonic()
        while True:
            chunk = self._s.read(4096)
            now = time.monotonic()
            if chunk:
                buf += chunk
                last = now
            elif buf and now - last >= idle:
                break
            if now - start >= max_wait:
                break
        return bytes(buf)

    def reset_input(self) -> None:
        self._s.reset_input_buffer()

    def close(self) -> None:
        self._s.close()


# ------------------------------------------------------------------ protocol


@dataclass(frozen=True)
class RadioIdentity:
    firmware: str
    model: str
    band: str
    raw: bytes

    @classmethod
    def parse(cls, raw: bytes) -> RadioIdentity:
        text = "".join(chr(b) if 0x20 <= b < 0x7F else "\x00" for b in raw)
        parts = [p for p in text.split("\x00") if p]
        firmware = next((p for p in parts if re.match(r"^\d{3}[0-9A-F]\.", p)), "")
        model = next((m.group(1) for p in parts if (m := re.search(r"\+([A-Z]{1,4}-[0-9A-Z]+)", p))), "")
        band = next((m.group(1) for p in parts if (m := re.search(r"\+(\d+-\d+)MHZ\+", p))), "")
        return cls(firmware, model, band, raw)

    def summary(self) -> str:
        return f"{self.model or '未知機型'}　{self.band + ' MHz' if self.band else ''}　韌體 {self.firmware or '?'}"


@dataclass(frozen=True)
class ReadResult:
    project: Project
    stream: bytes
    identity: RadioIdentity
    counts: Counts


@dataclass(frozen=True)
class WriteResult:
    checksum: int
    radio_log: str


def identity_mismatch(project: Project, identity: RadioIdentity) -> str | None:
    """Why ``project`` must not be written to the radio that reported ``identity``."""
    if not (identity.model and identity.band):
        return "無法辨識對講機的機型或頻段"
    dev = project.device
    if dev.model and dev.model != identity.model:
        return f"專案機型為 {dev.model}，對講機是 {identity.model}"
    band = dev.band_label.lstrip("F")
    if band and band != identity.band:
        return f"專案頻段為 {band} MHz，對講機是 {identity.band} MHz"
    return None


def clock_write_project(read: ReadResult) -> Project:
    """What to write back so that only the radio's clock changes: the project just read,
    provided it rebuilds the transfer that was read byte for byte (else RadioError)."""
    try:
        same = build_stream(read.project) == read.stream
    except CodeplugError as exc:
        raise RadioError(f"已中止，未寫入任何資料：讀回的資料無法寫回（{exc}）") from exc
    if not same:
        raise RadioError("已中止，未寫入任何資料：讀回的資料無法逐位元組重建，為了安全不寫回。")
    return read.project


def save_backup(result: ReadResult, directory: Path, kind: str, when: datetime | None = None) -> Path:
    """Keep what was read from the radio: the project (.F136) and the raw transfer (.bin)."""
    from . import f136 as f136_mod

    directory.mkdir(parents=True, exist_ok=True)
    stamp = (when or datetime.now()).strftime("%Y%m%d_%H%M%S")
    model = re.sub(r"[^0-9A-Za-z_-]", "_", result.identity.model or "radio")
    base = directory / f"{stamp}_{kind}_{model}"
    base.with_suffix(".bin").write_bytes(result.stream)
    path = base.with_suffix(".F136")
    path.write_bytes(f136_mod.serialize(result.project))
    return path


def command(verb: bytes, blocks: int, write_password: bytes = b"", read_password: bytes = b"") -> bytes:
    for pw in (write_password, read_password):
        if len(pw) > PASSWORD_SIZE:
            raise ValueError("密碼最多 6 bytes")
    pad = lambda pw: pw + bytes(PASSWORD_SIZE - len(pw))  # noqa: E731
    return (verb + struct.pack(">II", PART1_ADDR, blocks)
            + pad(write_password) + pad(read_password))


def _checksum(tail: bytes, tag: bytes) -> int:
    if tail[:len(tag)] != tag or len(tail) < len(tag) + 4:
        raise RadioError(f"對講機未回傳校驗碼（收到 {tail[:16]!r}）")
    return int(struct.unpack_from(">I", tail, len(tag))[0])


class RadioClient:
    REPLY_TIMEOUT = 5.0
    BLOCK_TIMEOUT = 6.0
    ACK_TIMEOUT = 10.0
    CHECKSUM_TIMEOUT = 15.0

    def __init__(self, transport: Transport, progress: Progress | None = None) -> None:
        self._t = transport
        self._progress = progress or (lambda done, total, text: None)

    def _first_reply(self, n: int) -> bytes:
        """The radio's answer to a command; silence gets a checklist instead of a byte count."""
        try:
            return self._t.read_exact(n, self.REPLY_TIMEOUT)
        except RadioTimeout as exc:
            if exc.received == 0:
                raise RadioTimeout(NO_REPLY, 0) from exc
            raise

    def read_codeplug(self) -> ReadResult:
        self._t.reset_input()
        self._progress(0, 1, "連線中…")
        self._t.write(command(READ_VERB, LEGACY_READ_BLOCKS))
        head = self._first_reply(len(READ_PREFIX) + 10)
        if head[:len(READ_PREFIX)] != READ_PREFIX:
            raise RadioError(f"對講機回應不符（{head[:12]!r}）。請確認機型、連線與寫頻線。")
        zones, channels, contacts, p1, p2 = struct.unpack_from("<5H", head, len(READ_PREFIX))
        counts = Counts(zones, channels, contacts)
        if (counts.part1_blocks, counts.part2_blocks) != (p1, p2):
            raise RadioError(f"對講機回報的區塊數 ({p1}+{p2}) 與計數不符，停止讀取")
        identity = RadioIdentity.parse(self._t.read_idle(0.3, 3.0))
        total = counts.total_blocks
        blocks: list[bytes] = []
        for i in range(total):
            self._progress(i, total, f"讀取區塊 {i + 1}/{total}")
            self._t.write(READ_REQUEST)
            blocks.append(self._t.read_exact(BLOCK, self.BLOCK_TIMEOUT))
        stream = b"".join(blocks)
        expected = _checksum(self._t.read_exact(len(CHECKSUM_READ) + 4, self.CHECKSUM_TIMEOUT),
                             CHECKSUM_READ)
        if sum(stream) & 0xFFFFFFFF != expected:
            raise RadioError("讀取資料的校驗和不符，資料可能在傳輸中損壞，請重試")
        self._progress(total, total, "讀取完成")
        try:
            project = parse_stream(stream, counts)
        except CodeplugError as exc:
            raise RadioError(f"讀到的資料格式不符：{exc}") from exc
        return ReadResult(project, stream, identity, counts)

    def write_codeplug(self, project: Project, date_text: str | None,
                       expect: Callable[[RadioIdentity], str | None] | None = None) -> WriteResult:
        """Write ``project``. ``expect`` may veto the write after seeing the radio's identity;
        nothing has been sent at that point, so the radio simply times out and keeps its data."""
        try:
            stream = build_stream(project, date_text)
        except CodeplugError as exc:
            raise RadioError(f"專案無法寫入：{exc}") from exc
        counts = Counts.of(project)
        total = counts.total_blocks
        self._t.reset_input()
        self._progress(0, total, "連線中…")
        self._t.write(command(WRITE_VERB, total))
        head = self._first_reply(len(WRITE_PREFIX))
        if head != WRITE_PREFIX:
            raise RadioError(f"對講機回應不符（{head!r}）。請確認機型、連線與寫頻線。")
        identity = RadioIdentity.parse(self._t.read_idle(0.3, 3.0))
        if expect is not None:
            reason = expect(identity)
            if reason:
                raise RadioError(f"已中止，未寫入任何資料：{reason}")
        for i in range(total):
            self._progress(i, total, f"寫入區塊 {i + 1}/{total}")
            self._t.write(stream[i * BLOCK:(i + 1) * BLOCK])
            if i < total - 1:
                try:
                    ack = self._t.read_exact(len(WRITE_ACK), self.ACK_TIMEOUT)
                except RadioTimeout:
                    ack = b"(no reply)"
                if ack != WRITE_ACK:
                    raise RadioError(f"第 {i + 1} 塊未獲確認（{ack!r}）。對講機不會套用不完整的資料，"
                                     "原本的設定仍保留。")
        self._progress(total, total, "等待對講機確認…")
        got = _checksum(self._t.read_exact(len(CHECKSUM_WRITE) + 4, self.CHECKSUM_TIMEOUT),
                        CHECKSUM_WRITE)
        log = self._t.read_idle(1.0, 20.0).decode("ascii", errors="replace")
        if got != sum(stream) & 0xFFFFFFFF:
            raise RadioError(
                "對講機收到的資料校驗和不符，已套用的設定可能損壞。請重新寫入，或寫回寫入前的備份。")
        self._progress(total, total, "寫入完成")
        return WriteResult(got, log)
