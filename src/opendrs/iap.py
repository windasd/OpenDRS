"""Firmware upgrade through the radio's IAP bootloader (replaces the vendor ``IAP+Flash.exe``).

Protocol, from static analysis of IAP+Flash.exe 1.00.0047 (VB6: the "开始下载" button,
Timer1 = reply parser, Timer2 = page sender) and cross-checked with the radio firmware's
own radio-to-radio clone code (unitA 0x03020cd0..0x030210d8), which speaks the same
protocol as the sending side:

    PC -> "Erase      " (11 bytes) + target address u32 BE + last page index u32 BE
    RF -> text containing "ok" (the vendor tool waits for "ok", the clone code for "IAP")
    then for page = 0 .. last page index (inclusive, so one more page than the index):
    PC -> 2048 bytes of the image, the last page padded with 0x00
    RF -> "kyd"
    after the last page:
    RF -> "ChecksumI" + u32 BE byte sum of every page sent (padding included)

The last page index is ``len(image) // 2048`` exactly as the vendor tool computes it
(an image that is an exact multiple of 2048 therefore gets one extra page of zeros).

Verified by running the vendor tool itself against a simulated bootloader (Frida hooks on
its serial API, no hardware): for the vendor firmware and for two synthetic images (one
with a short last page of non-zero data, one an exact multiple of 2048) the vendor tool's
output was byte-identical to :meth:`IapImage.stream`, and the vendor tool accepted the
simulated replies ("IAP Erase ok", "kyd", "ChecksumI"+sum) and reported
"IAP Checksum right".

Verified on hardware on 2026-10-09: the user flashed the vendor image and the record-read patched
image (mkpatch_record.py) onto a DR-8500UV (BK4819) with this module; both upgrades completed with
matching checksums and the radio ran the new image (recording download worked after the second).
So the real bootloader answers the erase with text containing "ok", each page with "kyd" and the end
with "ChecksumI" + sum, as modelled here (the exact erase reply text was not recorded).

The bootloader must already be running: the vendor tool sends nothing to put the radio
into IAP mode, the user does that on the radio. Once "Erase" is accepted the radio has
no application until every page has arrived — an interrupted upgrade leaves it in the
bootloader, from where the upgrade can simply be repeated.
"""

from __future__ import annotations

import time
from collections.abc import Callable
from dataclasses import dataclass

from .radio_link import RadioError, RadioTimeout, Transport

PAGE_SIZE = 2048
ERASE_VERB = b"Erase      "
ERASE_ACK = b"ok"
PAGE_ACK = b"kyd"
CHECKSUM_TAG = b"ChecksumI"
PAD_BYTE = 0x00
MAX_IMAGE = 4 * 1024 * 1024
DEFAULT_ADDRESS = 0

Progress = Callable[[int, int, str], None]


class IapError(RadioError):
    """Upgrade failure. ``erase_sent``: the erase command went out, so the radio's
    application may already be gone (it stays in the bootloader, ready for a retry)."""

    def __init__(self, message: str, erase_sent: bool) -> None:
        super().__init__(message)
        self.erase_sent = erase_sent


class _Stopped(Exception):
    """The user pressed Stop while waiting for a reply."""


RETRY = ("對講機目前可能沒有可用的韌體：請讓它保持在升級模式並重新執行升級；若仍失敗，"
         "將對講機關機後重新進入升級模式再試，或改用原廠 IAP 工具。")


@dataclass(frozen=True)
class IapImage:
    pages: tuple[bytes, ...]
    size: int
    address: int = DEFAULT_ADDRESS

    @classmethod
    def from_bytes(cls, data: bytes, address: int = DEFAULT_ADDRESS, pad: int = PAD_BYTE) -> IapImage:
        if not data:
            raise ValueError("韌體檔是空的")
        if len(data) > MAX_IMAGE:
            raise ValueError(f"韌體檔太大（{len(data):,} bytes）")
        if not 0 <= address <= 0xFFFFFFFF:
            raise ValueError("位址超出範圍")
        last = len(data) // PAGE_SIZE
        padded = data + bytes([pad]) * ((last + 1) * PAGE_SIZE - len(data))
        pages = tuple(padded[i * PAGE_SIZE:(i + 1) * PAGE_SIZE] for i in range(last + 1))
        return cls(pages, len(data), address)

    @property
    def last_page(self) -> int:
        return len(self.pages) - 1

    @property
    def checksum(self) -> int:
        return sum(sum(p) for p in self.pages) & 0xFFFFFFFF

    def erase_command(self) -> bytes:
        return ERASE_VERB + self.address.to_bytes(4, "big") + self.last_page.to_bytes(4, "big")

    def stream(self) -> bytes:
        """Every byte the PC sends, in order (used to compare with the vendor tool)."""
        return self.erase_command() + b"".join(self.pages)


@dataclass(frozen=True)
class IapResult:
    checksum: int
    pages: int
    erase_reply: str


class IapClient:
    ERASE_TIMEOUT = 60.0
    PAGE_TIMEOUT = 10.0
    CHECKSUM_TIMEOUT = 10.0
    POLL = 0.25  # longest single read, so Stop is noticed quickly

    def __init__(self, transport: Transport, progress: Progress | None = None,
                 cancelled: Callable[[], bool] | None = None) -> None:
        self._t = transport
        self._progress = progress or (lambda done, total, text: None)
        self._cancelled = cancelled or (lambda: False)
        self._pending = bytearray()
        self.erase_sent = False

    def _wait_for(self, token: bytes, timeout: float, extra: int = 0) -> bytes:
        """Consume replies up to and including ``token`` (case-insensitive, like the vendor
        tool, which only looks for substrings) plus ``extra`` bytes after it; anything that
        arrived later stays buffered for the next step."""
        deadline = time.monotonic() + timeout
        while True:
            at = bytes(self._pending).lower().find(token.lower())
            end = at + len(token) + extra
            if at >= 0 and len(self._pending) >= end:
                taken = bytes(self._pending[:end])
                del self._pending[:end]
                return taken
            if self._cancelled():
                raise _Stopped
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RadioTimeout(f"等待 {token.decode()} 逾時（收到 {bytes(self._pending)[-40:]!r}）",
                                   len(self._pending))
            self._pending += self._t.read_idle(0.05, min(remaining, self.POLL))

    def upgrade(self, image: IapImage) -> IapResult:
        self.erase_sent = False
        try:
            return self._upgrade(image)
        except _Stopped as exc:
            raise IapError("已停止。" + (RETRY if self.erase_sent else "未送出任何資料，對講機的韌體未變動。"),
                           self.erase_sent) from exc
        except OSError as exc:  # serial errors (cable pulled, adapter reset) are OSErrors
            raise IapError(f"與對講機的連線中斷（{exc}）。" + (RETRY if self.erase_sent else "未送出清除指令。"),
                           self.erase_sent) from exc

    def _upgrade(self, image: IapImage) -> IapResult:
        total = len(image.pages)
        self._t.reset_input()
        self._pending.clear()
        if self._cancelled():
            raise _Stopped
        self._progress(0, total, "傳送清除指令…")
        self.erase_sent = True
        self._t.write(image.erase_command())
        self._progress(0, total, f"等待對講機清除（最多 {self.ERASE_TIMEOUT:.0f} 秒）…")
        try:
            reply = self._wait_for(ERASE_ACK, self.ERASE_TIMEOUT)
        except RadioTimeout as exc:
            raise IapError("清除指令已送出，但沒有收到對講機的確認，無法確定它是否已開始清除。"
                           "請確認對講機已進入升級（IAP）模式、寫頻線已插緊、連接埠正確；"
                           f"若對講機仍停在升級模式，可重新執行升級。\n（{exc}）", True) from exc
        erase_text = reply.decode("ascii", errors="replace")
        for index, page in enumerate(image.pages):
            if self._cancelled():
                raise _Stopped
            self._progress(index, total, f"寫入第 {index + 1}/{total} 頁")
            self._t.write(page)
            if index == image.last_page:
                break  # its "kyd" (if any) is consumed while waiting for the checksum
            try:
                self._wait_for(PAGE_ACK, self.PAGE_TIMEOUT)
            except RadioTimeout as exc:
                raise IapError(f"第 {index + 1}/{total} 頁沒有收到確認。{RETRY}\n（{exc}）", True) from exc
        self._progress(total, total, "等待對講機校驗…")
        try:
            tail = self._wait_for(CHECKSUM_TAG, self.PAGE_TIMEOUT + self.CHECKSUM_TIMEOUT, extra=4)
        except RadioTimeout as exc:
            raise IapError(f"所有頁面已送出，但對講機沒有回傳校驗碼。{RETRY}\n（{exc}）", True) from exc
        got = int.from_bytes(tail[-4:], "big")
        if got != image.checksum:
            raise IapError(f"校驗和不符（對講機 {got:#010x}，應為 {image.checksum:#010x}），資料可能在傳輸中損壞，"
                           f"請勿直接開機使用。{RETRY}", True)
        self._progress(total, total, "升級完成")
        return IapResult(got, total, erase_text)
