"""Supported radios: Kydera DR-8500UV / DRS-8500 and the re-badged HORA DH-98VU."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from opendrs import f136, hardware

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"


def test_supported_list_names_the_dh98vu() -> None:
    names = hardware.supported_names()
    assert names == "Kydera DR-8500UV／Kydera DRS-8500／HORA DH-98VU"
    assert "DRS-8500" in hardware.supported_text() and "DR8600" in hardware.supported_text()


def test_dh98vu_project_reports_the_shared_identifiers() -> None:
    dev = f136.load(DH98).device
    assert (dev.model, dev.model_number) == (hardware.REPORTED_MODEL, hardware.MODEL_NUMBER)
    assert hardware.known_as(dev.model) == "Kydera DR-8500UV／HORA DH-98VU"
    assert hardware.known_as("XYZ-1") == "" and hardware.known_as("") == ""


def test_about_and_device_page_show_it(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtWidgets import QApplication

    from opendrs.gui.about_dialog import about_text

    app = QApplication.instance() or QApplication([])
    assert "HORA DH-98VU" in about_text(tmp_path)
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    from opendrs.gui.main_window import MainWindow

    target = tmp_path / "dh98.F136"
    target.write_bytes(DH98.read_bytes())
    win = MainWindow()
    assert win.open_path(target)
    assert win._dev_labels["model"].text() == "DRS-8500（Kydera DR-8500UV／HORA DH-98VU）"
    win.close()
    app.processEvents()
