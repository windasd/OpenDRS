"""MainWindow mixin: 工具 › 檢查更新…, the daily check at start-up, and installing an update."""

from __future__ import annotations

import logging
from collections.abc import Callable
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path

from .. import __version__, update
from ..settings import default_dir, save_settings
from ..update import PreparedUpdate, Release
from .update_dialog import CheckThread, Fetch, UpdateDialog

log = logging.getLogger(__name__)
UPDATE_LOG = Path("logs") / "update.log"


class UpdateOpsMixin:
    _known_release: Release | None = None
    _auto_check: CheckThread | None = None

    def check_for_updates(self, fetch: Fetch = update.fetch_latest,
                          target: Path | None = None) -> UpdateDialog:
        dialog = UpdateDialog(self, fetch=fetch, target=target if target is not None else update.install_dir(),
                              on_install=self.apply_update, auto_check=self._settings.check_updates,
                              on_auto_check_changed=self._set_auto_check)
        dialog.open()
        if self._known_release is not None:
            dialog.show_release(self._known_release)
        else:
            dialog.check()
        return dialog

    def _set_auto_check(self, enabled: bool) -> None:
        self._settings = replace(self._settings, check_updates=enabled)
        save_settings(self._settings)

    def start_update_check(self, fetch: Fetch = update.fetch_latest, now: datetime | None = None) -> bool:
        """The automatic check at start-up: at most daily, and silent unless there is a newer version."""
        now = now or datetime.now(UTC)
        if not self._settings.check_updates or not update.check_due(self._settings.last_update_check, now):
            return False
        self._auto_check = CheckThread(fetch)
        self._auto_check.found.connect(lambda release: self._auto_found(release, now))
        self._auto_check.failed.connect(lambda message: log.info("automatic update check: %s", message))
        self._auto_check.start()
        return True

    def _auto_found(self, release: Release, when: datetime) -> None:
        self._settings = replace(self._settings, last_update_check=when.isoformat())
        save_settings(self._settings)
        self._known_release = release
        if release.is_newer_than(__version__):
            self._a_update.setText(f"檢查更新…（有新版本 {release.tag}）")
            self.statusBar().showMessage(f"有新版本 {release.tag} 可以更新：工具 › 檢查更新…")

    def apply_update(self, prepared: PreparedUpdate,
                     launcher: Callable[[PreparedUpdate, Path], None] = update.launch) -> bool:
        """Close the window (asking about unsaved changes as usual), then hand over to the updater."""
        if not self.close():
            return False
        launcher(prepared, default_dir() / UPDATE_LOG)
        log.info("update to %s handed over to the updater", prepared.version)
        return True
