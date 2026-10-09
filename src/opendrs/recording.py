"""Download call recordings from the radio (replaces the vendor ``C:\\wav\\Wav.exe``).

Protocol, from Wav.exe 1.00.0072 (VB6; captured byte for byte by running it against a
simulated radio) and the radio firmware (unitA FUN_030272fc / FUN_03034e90 /
FUN_03027e1c / FUN_03027f24, FUN_03033f4c for the record index):

* **List** ("Read List"): a binary frame ``68 58 FF 01 <csum16> <len u16 LE = 2> <n u16 BE> 10``;
  the checksum is the 16-bit one's-complement sum of the frame (BE words, checksum field 0;
  same algorithm as firmware FUN_03026f4c). The firmware's handler for it (unitA 0x030272c4,
  not routed — see below) reads ``n`` as a record number and answers that record's line.
  Wav.exe sends n = 100, 99, 98, ...; this module asks for 1..total instead.
* **Record line**: ``#<no>#<total>#<R|S>#<id %08d>#<length bytes>#YYYY/MM/DD HH:MM``
  (R = received, S = sent).
* **Read** ("Read *.wav"): ``"Record Read"`` + record number u32 BE + pages u32 BE
  (Wav.exe sends length // 4050 + 1; the radio ignores it and uses the record length).
  The radio answers with the record line then ``"Record Read"``; every ``"Read"`` then
  returns 4050 bytes (450 AMBE+2 frames of 9 bytes = 9 s), ceil(length / 4050) blocks,
  followed by ``"ChecksumR"`` + u32 BE byte sum of the blocks sent.

The audio is AMBE+2 (3600 bit/s with FEC, 72 bits per 20 ms frame). Decoding needs the
vendor-supplied DVSI ``ambep2.exe`` (proprietary, licensed — it is run, never bundled):
its input is one byte (0/1) per bit, MSB first; its output is 16-bit 8 kHz mono PCM,
which is wrapped into a WAV here (Wav.exe used ffmpeg for that).

Caution: in the analysed firmware build (308E.B6) the PC receive loop only forwards
commands starting with "F" (and 0x68 frames with 0x21/0x88), so neither "Record Read"
nor the 0x58 list frame reaches the handlers. Tried on the user's radio (308E.BBKTARSCB.003,
the same build: the firmware writes "BK" into its ID at boot for the BK4819 RF chip)
on 2026-10-09: no byte came back to either command while a codeplug read worked, as the
analysis predicts (a routed "Record Read" always answers "Record Read", even for a missing
record). Both commands are read-only.

The patched image DR8500UV_B6_record_read.bin (mkpatch_record.py, 30 bytes) wires both handlers;
flashed on the user's BK4819 radio on 2026-10-09 it listed and downloaded 25 recordings (up to
5 blocks each) with matching checksums and speech-like audio after decoding.
"""

from __future__ import annotations

import re
import subprocess
import tempfile
import time
import wave
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .radio_link import READ_REQUEST, RadioError, RadioTimeout, Transport

RECORD_VERB = b"Record Read"
CHECKSUM_READ = b"ChecksumR"
BLOCK = 4050
FRAME_BYTES = 9  # one AMBE+2 frame (72 bits with FEC) = 20 ms
SAMPLE_RATE = 8000
LIST_CMD = 0x58
DEFAULT_LIST_MAX = 100
MAX_RECORDS = 200  # Wav.exe's grid limit
DEFAULT_AMBEP2 = Path(r"C:\wav\ambep2.exe")
LINE = re.compile(rb"#(\d+)#(\d+)#([RS])#(\d{1,10})#(\d+)#(\d{4}/\d{2}/\d{2} \d{2}:\d{2})")

Progress = Callable[[int, int, str], None]


class RecordingError(RadioError):
    pass


def checksum16(data: bytes) -> int:
    """16-bit one's-complement checksum (big-endian words, odd byte padded with 0)."""
    total = 0
    for i in range(0, len(data), 2):
        total += (data[i] << 8) | (data[i + 1] if i + 1 < len(data) else 0)
        total = (total & 0xFFFF) + (total >> 16)
    return ~total & 0xFFFF


def h_frame(cmd: int, payload: bytes) -> bytes:
    """``68 cmd FF 01 <csum16> <len u16 LE> payload 10``."""
    body = bytearray([0x68, cmd, 0xFF, 0x01, 0, 0]) + len(payload).to_bytes(2, "little") + payload + b"\x10"
    body[4:6] = checksum16(bytes(body)).to_bytes(2, "big")
    return bytes(body)


def list_request(remaining: int) -> bytes:
    return h_frame(LIST_CMD, remaining.to_bytes(2, "big"))


def record_read_command(number: int, pages: int) -> bytes:
    return RECORD_VERB + number.to_bytes(4, "big") + pages.to_bytes(4, "big")


@dataclass(frozen=True)
class RecordInfo:
    number: int
    total: int
    direction: str  # "R" received, "S" sent
    call_id: int
    length: int  # bytes of AMBE+2 data
    time: str  # "YYYY/MM/DD HH:MM" as the radio reports it

    @classmethod
    def parse(cls, text: bytes) -> RecordInfo | None:
        m = LINE.search(text)
        if not m:
            return None
        return cls(int(m.group(1)), int(m.group(2)), m.group(3).decode(), int(m.group(4)), int(m.group(5)),
                   m.group(6).decode())

    def line(self) -> bytes:
        return (f"#{self.number}#{self.total}#{self.direction}#{self.call_id:08d}#{self.length}"
                f"#{self.time}").encode("ascii")

    @property
    def blocks(self) -> int:
        """Blocks the radio sends (it always sends at least one)."""
        return max(1, -(-self.length // BLOCK))

    @property
    def pages_field(self) -> int:
        """What Wav.exe puts in the command's page field (ignored by the radio)."""
        return self.length // BLOCK + 1

    @property
    def seconds(self) -> float:
        return self.length / FRAME_BYTES * 0.02

    @property
    def stem(self) -> str:
        """File name: the radio's time, then Wav.exe's ``<no>_<R|S><id 8 digits>``. The time
        keeps a later call stored under the same record number from replacing an old file."""
        when = self.time.replace("/", "").replace(":", "").replace(" ", "_")
        return f"{when}_{self.number}_{self.direction}{self.call_id:08d}"


class RecordClient:
    LIST_TIMEOUT = 1.5
    REPLY_TIMEOUT = 5.0
    BLOCK_TIMEOUT = 6.0
    CHECKSUM_TIMEOUT = 6.0

    def __init__(self, transport: Transport, progress: Progress | None = None,
                 cancelled: Callable[[], bool] | None = None) -> None:
        self._t = transport
        self._progress = progress or (lambda done, total, text: None)
        self._cancelled = cancelled or (lambda: False)
        self._pending = bytearray()
        self.list_stopped = False  # the last list was cut short by Stop

    def _fill(self, until: Callable[[bytes], bool], timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while not until(bytes(self._pending)):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            self._pending += self._t.read_idle(0.05, min(remaining, 0.25))
        return True

    def _take_line(self, timeout: float) -> RecordInfo | None:
        if not self._fill(lambda b: LINE.search(b) is not None, timeout):
            return None
        m = LINE.search(bytes(self._pending))
        assert m is not None
        info = RecordInfo.parse(m.group(0))
        del self._pending[:m.end()]
        return info

    def _take(self, n: int, timeout: float) -> bytes:
        if not self._fill(lambda b: len(b) >= n, timeout):
            raise RadioTimeout(f"等待對講機回應逾時（預期 {n} bytes，收到 {len(self._pending)}）", len(self._pending))
        data = bytes(self._pending[:n])
        del self._pending[:n]
        return data

    def _take_token(self, token: bytes, timeout: float, extra: int = 0) -> bytes | None:
        def ready(b: bytes) -> bool:
            at = b.find(token)
            return at >= 0 and len(b) >= at + len(token) + extra
        if not self._fill(ready, timeout):
            return None
        at = bytes(self._pending).find(token)
        end = at + len(token) + extra
        taken = bytes(self._pending[at:end])
        del self._pending[:end]
        return taken

    # ------------------------------------------------------------------ list
    def list_records(self, max_count: int = DEFAULT_LIST_MAX) -> list[RecordInfo]:
        """Records 1..total, one list frame each. The firmware's (unrouted) handler at
        unitA 0x030272c4 reads the frame payload as the record number and answers that
        record's line; record 1's line also gives the total."""
        self._t.reset_input()
        self._pending.clear()
        found: list[RecordInfo] = []
        number, last = 1, min(max_count, MAX_RECORDS)
        while number <= last and not self._cancelled():
            self._progress(number - 1, last, f"讀取清單 {number}/{last}…")
            self._t.write(list_request(number))
            # the first answer may take a while (Wav.exe waits about 5 s); later ones come quickly
            info = self._take_line(self.LIST_TIMEOUT if found else self.REPLY_TIMEOUT)
            if info is None or info.number != number:
                break
            found.append(info)
            last = min(last, info.total)
            number += 1
        if self._cancelled() and not found:
            raise RecordingError("已停止。")
        if not found:
            raise RecordingError("對講機沒有回應錄音清單指令。這台對講機的韌體可能不支援從電腦讀取錄音"
                                 "（DR-8500UV／DH-98VU 原廠韌體不支援，需先刷入錄音讀取修補版）；也請確認連接埠與寫頻線。")
        self.list_stopped = self._cancelled()
        self._progress(len(found), len(found), f"共 {len(found)} 筆錄音")
        return found

    # ------------------------------------------------------------------ read
    def read_record(self, number: int, pages: int = 1) -> tuple[RecordInfo, bytes]:
        """One record: the radio's line for it and its AMBE+2 data (``length`` bytes).
        ``pages`` only fills the command like Wav.exe does; the radio decides the count."""
        self._t.reset_input()
        self._pending.clear()
        self._t.write(record_read_command(number, pages))
        header = self._take_line(self.REPLY_TIMEOUT)
        if header is None:
            raise RecordingError(f"對講機沒有回傳第 {number} 筆錄音的資訊（錄音可能不存在，或韌體不支援）。")
        if self._take_token(RECORD_VERB, self.REPLY_TIMEOUT) is None:
            raise RecordingError("對講機回應不完整（缺少 Record Read），請重試。")
        if header.number != number:
            raise RecordingError(f"要求第 {number} 筆，對講機回傳第 {header.number} 筆，已停止。")
        blocks: list[bytes] = []
        for i in range(header.blocks):
            if self._cancelled():
                raise RecordingError("已停止。")
            self._progress(i, header.blocks, f"第 {number} 筆：{i + 1}/{header.blocks}")
            self._t.write(READ_REQUEST)
            blocks.append(self._take(BLOCK, self.BLOCK_TIMEOUT))
        tail = self._take_token(CHECKSUM_READ, self.CHECKSUM_TIMEOUT, extra=4)
        if tail is None:
            raise RecordingError("對講機沒有回傳校驗碼，資料可能不完整，請重試。")
        data = b"".join(blocks)
        if int.from_bytes(tail[-4:], "big") != sum(data) & 0xFFFFFFFF:
            raise RecordingError(f"第 {number} 筆錄音的校驗和不符，資料可能在傳輸中損壞，請重試。")
        self._progress(header.blocks, header.blocks, f"第 {number} 筆完成")
        return header, data[:header.length]


# ---------------------------------------------------------------------- audio
def to_bit_stream(data: bytes) -> bytes:
    """ambep2's input: one byte (0/1) per bit, MSB first, 72 per frame. Whole frames only."""
    usable = len(data) - len(data) % FRAME_BYTES
    return bytes((b >> (7 - k)) & 1 for b in data[:usable] for k in range(8))


def decode_ambe(data: bytes, ambep2: Path, timeout: float = 30.0) -> bytes:
    """16-bit 8 kHz mono PCM, decoded by the vendor's ambep2.exe."""
    if not Path(ambep2).is_file():
        raise RecordingError(f"找不到 ambep2.exe（{ambep2}），無法把錄音轉成聲音")
    with tempfile.TemporaryDirectory(prefix="opendrs_rec_", ignore_cleanup_errors=True) as tmp:
        (Path(tmp) / "rec.bit").write_bytes(to_bit_stream(data))
        try:
            # bare names in cwd: ambep2 is an ANSI program and cannot open paths with
            # characters outside the system code page (e.g. in %TEMP%)
            subprocess.run([str(ambep2), "-dec", "rec.bit", "rec.dat"], cwd=tmp, capture_output=True,
                           timeout=timeout, check=True,
                           creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        except subprocess.CalledProcessError as exc:
            detail = (exc.stderr or b"").decode("cp950", "replace").strip().splitlines()[-1:] or [""]
            raise RecordingError(f"ambep2.exe 執行失敗（結束碼 {exc.returncode}）{detail[0]}") from exc
        except (OSError, subprocess.SubprocessError) as exc:
            raise RecordingError(f"ambep2.exe 執行失敗：{exc}") from exc
        pcm = Path(tmp) / "rec.dat"
        if not pcm.is_file():
            raise RecordingError("ambep2.exe 沒有產生輸出")
        return pcm.read_bytes()


def write_wav(path: Path, pcm: bytes) -> Path:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm[:len(pcm) - len(pcm) % 2])
    return path
