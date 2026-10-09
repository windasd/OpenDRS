"""Background thread for a built-in IAP firmware upgrade."""

from __future__ import annotations

import logging
from collections.abc import Callable

from PySide6.QtCore import QThread, Signal

from ..iap import IapClient, IapError, IapImage
from ..radio_link import RadioError, Transport

log = logging.getLogger(__name__)


class IapWorker(QThread):
    progress = Signal(int, int, str)
    succeeded = Signal(object)  # IapResult
    failed = Signal(str, bool)  # message, erase command already sent

    def __init__(self, factory: Callable[[], Transport], image: IapImage) -> None:
        super().__init__()
        self._factory, self._image = factory, image
        self._cancel = False

    def cancel(self) -> None:
        self._cancel = True

    def run(self) -> None:
        try:
            transport = self._factory()
        except Exception as exc:  # noqa: BLE001 - e.g. the port is busy or gone
            self.failed.emit(str(exc) if isinstance(exc, RadioError) else f"無法開啟連接埠：{exc}", False)
            return
        client = IapClient(transport, self.progress.emit, cancelled=lambda: self._cancel)
        try:
            self.succeeded.emit(client.upgrade(self._image))
        except IapError as exc:
            self.failed.emit(str(exc), exc.erase_sent)
        except Exception as exc:  # noqa: BLE001 - report anything unexpected to the user
            log.exception("IAP upgrade failed")
            self.failed.emit(f"未預期的錯誤：{exc}", client.erase_sent)
        finally:
            transport.close()
