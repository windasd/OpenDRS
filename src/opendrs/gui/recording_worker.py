"""Background thread for listing and downloading call recordings."""

from __future__ import annotations

import logging
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from PySide6.QtCore import QThread, Signal

from ..radio_link import RadioError, Transport
from ..recording import RecordClient, RecordInfo, RecordingError, decode_ambe, write_wav

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class Saved:
    info: RecordInfo
    raw: Path
    wav: Path | None  # None when ambep2.exe was not available
    note: str = ""


class RecordingWorker(QThread):
    progress = Signal(int, int, str)
    listed = Signal(object)  # list[RecordInfo]
    saved = Signal(object)  # Saved
    failed = Signal(str)

    def __init__(self, factory: Callable[[], Transport], mode: str, out_dir: Path, ambep2: Path,
                 numbers: list[tuple[int, int]] | None = None, max_count: int = 100,
                 pause: float = 0.5) -> None:
        """``mode``: "list", "download" (``numbers`` = (record, pages) pairs) or "all"."""
        super().__init__()
        self._factory, self._mode, self._out, self._ambep2 = factory, mode, out_dir, ambep2
        self._numbers, self._max, self._pause = list(numbers or []), max_count, pause
        self._cancel = False

    def cancel(self) -> None:
        self._cancel = True

    def run(self) -> None:
        try:
            transport = self._factory()
        except Exception as exc:  # noqa: BLE001 - e.g. the port is busy or gone
            self.failed.emit(str(exc) if isinstance(exc, RadioError) else f"無法開啟連接埠：{exc}")
            return
        client = RecordClient(transport, self.progress.emit, cancelled=lambda: self._cancel)
        try:
            if self._mode == "list":
                self.listed.emit(client.list_records(self._max))
            elif self._mode == "download":
                for i, (number, pages) in enumerate(self._numbers):
                    self._between(i)
                    self._save(*client.read_record(number, pages))
            else:
                self._download_all(client)
        except RadioError as exc:
            self.failed.emit(str(exc))
        except Exception as exc:  # noqa: BLE001 - report anything unexpected to the user
            log.exception("recording transfer failed")
            self.failed.emit(f"未預期的錯誤：{exc}")
        finally:
            transport.close()

    def _download_all(self, client: RecordClient) -> None:
        """Record Read 1..total, without the list command (total comes with record 1)."""
        first, data = client.read_record(1)
        self._save(first, data)
        for number in range(2, min(first.total, 999) + 1):
            self._between(number)
            self._save(*client.read_record(number))

    def _between(self, index: int) -> None:
        """Stop if asked; give the radio a moment back in its command loop between records."""
        if self._cancel:
            raise RecordingError("已停止。")
        if index:
            time.sleep(self._pause)

    def _free_stem(self, info: RecordInfo, data: bytes) -> str:
        """``info.stem``, or with a suffix when a different recording already has that name."""
        stem, n = info.stem, 1
        while (self._out / f"{stem}.ambe").exists() and (self._out / f"{stem}.ambe").read_bytes() != data:
            n += 1
            stem = f"{info.stem}_{n}"
        return stem

    def _save(self, info: RecordInfo, data: bytes) -> None:
        try:
            self._out.mkdir(parents=True, exist_ok=True)
            stem = self._free_stem(info, data)
            raw = self._out / f"{stem}.ambe"
            raw.write_bytes(data)
        except OSError as exc:  # keep going with the other records
            self.failed.emit(f"第 {info.number} 筆無法儲存到 {self._out}：{exc}")
            return
        if not self._ambep2.is_file():
            self.saved.emit(Saved(info, raw, None, "找不到 ambep2.exe，只保存原始資料"))
            return
        try:
            wav = write_wav(self._out / f"{stem}.wav", decode_ambe(data, self._ambep2))
        except (RecordingError, OSError) as exc:
            self.saved.emit(Saved(info, raw, None, str(exc)))
            return
        self.saved.emit(Saved(info, raw, wav))
