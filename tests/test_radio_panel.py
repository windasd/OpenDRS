from __future__ import annotations

import os
import struct
from dataclasses import replace
from datetime import datetime
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtWidgets")
from PySide6.QtWidgets import QApplication, QMessageBox  # noqa: E402

from opendrs import f136  # noqa: E402
from opendrs.codeplug import DATE_FORMAT, Counts  # noqa: E402
from opendrs.gui.radio_panel import SIMULATED, RadioPanel, write_summary  # noqa: E402
from opendrs.radio_link import (  # noqa: E402
    RadioClient,
    RadioError,
    RadioIdentity,
    clock_write_project,
    identity_mismatch,
    save_backup,
)
from opendrs.radio_sim import SimulatedRadio  # noqa: E402

CAP = Path(__file__).parent / "fixtures" / "capture"


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


@pytest.fixture
def radio() -> SimulatedRadio:
    return SimulatedRadio.with_stream((CAP / "read_stream.bin").read_bytes(), Counts(4, 27, 360))


def run(panel: RadioPanel, app: QApplication, mode: str, project=None) -> list:  # noqa: ANN001
    done: list = []
    panel.finished.connect(lambda outcome, error: done.append((outcome, error)))
    panel.port_box.setCurrentIndex(panel.port_box.findData(SIMULATED))
    assert panel.start(mode, project)
    assert not panel.start(mode, project)  # one transfer at a time
    panel.wait()
    app.processEvents()
    app.processEvents()
    assert len(done) == 1 and not panel.is_running()
    return done


def make_panel(tmp_path: Path, radio: SimulatedRadio) -> RadioPanel:
    return RadioPanel(lambda: tmp_path, simulated_radio=radio)


def test_panel_read_with_backup(app: QApplication, radio: SimulatedRadio, tmp_path: Path) -> None:
    panel = make_panel(tmp_path, radio)
    [(outcome, error)] = run(panel, app, "read")
    assert error == "" and outcome.read.counts == Counts(4, 27, 360)
    backup = outcome.backup
    assert backup.suffix == ".F136" and backup.with_suffix(".bin").read_bytes() == (
        CAP / "read_stream.bin").read_bytes()
    assert f136.load(backup).channels == outcome.read.project.channels
    assert panel.status.text() == "讀取完成" and not panel.progress.isVisible()


def test_panel_write_backs_up_then_writes(app: QApplication, radio: SimulatedRadio,
                                          tmp_path: Path) -> None:
    project = f136.load(CAP / "written_project.F136")
    edited = project.with_channel(0, project.channels[0].with_freq_a(145_500_000))
    assert "頻道 27" in write_summary(edited)
    panel = make_panel(tmp_path, radio)
    [(outcome, error)] = run(panel, app, "write", edited)
    assert error == "" and outcome.mode == "write"
    assert radio.commits == 1
    assert "before_write" in outcome.backup.name
    back = RadioClient(radio).read_codeplug()
    assert back.project.channels[0].freq_a_hz == 145_500_000
    stamp = back.stream[0x60:0x70].rstrip(b"\x00").decode()
    assert datetime.strptime(stamp, "%Y/%m/%d %H:%M")
    assert any("備份完成" in line for line in panel.log)


def test_panel_write_aborts_on_band_mismatch(app: QApplication, radio: SimulatedRadio,
                                             tmp_path: Path) -> None:
    project = f136.load(CAP / "written_project.F136")
    img = bytearray(project.image)
    img[0x50:0x58] = b"F400-480"
    wrong = replace(project, image=bytes(img))
    panel = make_panel(tmp_path, radio)
    [(outcome, error)] = run(panel, app, "write", wrong)
    assert outcome is None and radio.commits == 0
    assert "未寫入任何資料" in error and "400-480" in error
    assert panel.port_box.isEnabled()


def test_port_list_keeps_preferred_choice(app: QApplication, tmp_path: Path,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    from types import SimpleNamespace

    import opendrs.gui.radio_panel as rp

    ports = [SimpleNamespace(device="COM3", description="USB"),
             SimpleNamespace(device="COM4", description="Prolific PL2303GC")]
    monkeypatch.setattr(rp, "list_ports", lambda: ports)
    panel = RadioPanel(lambda: tmp_path, preferred_port="COM4")
    assert panel.port() == "COM4"
    panel.port_box.setCurrentIndex(0)
    panel.refresh_ports()  # refreshing keeps the current selection
    assert panel.port() == "COM3"
    monkeypatch.setattr(rp, "list_ports", lambda: [])
    panel.refresh_ports()
    assert panel.port() == SIMULATED and "找不到序列埠" in panel.status.text()


def test_identity_mismatch_rules() -> None:
    project = f136.load(CAP / "written_project.F136")
    ok = RadioIdentity("308E", "DRS-8500", "136-480", b"")
    assert identity_mismatch(project, ok) is None
    assert "機型" in identity_mismatch(project, replace(ok, model="DRS-8600"))
    assert "頻段" in identity_mismatch(project, replace(ok, band="400-480"))
    assert "無法辨識" in identity_mismatch(project, replace(ok, model=""))


def test_save_backup_names(radio: SimulatedRadio, tmp_path: Path) -> None:
    result = RadioClient(radio).read_codeplug()
    path = save_backup(result, tmp_path / "b", "read", when=datetime(2026, 10, 8, 18, 0, 0))
    assert path.name == "20261008_180000_read_DRS-8500.F136"
    assert path.with_suffix(".bin").exists()


def test_main_window_radio_flows(app: QApplication, radio: SimulatedRadio, tmp_path: Path,
                                 monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.main_window import MainWindow

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    win = MainWindow()
    assert not win.has_project()
    result = RadioClient(radio).read_codeplug()
    win.load_from_radio(result)
    assert win.has_project() and win.is_dirty()
    assert "從對講機讀取（DRS-8500" in win.windowTitle()
    assert win._tabs.tabText(1) == "頻道（27）"
    too_many = replace(win._project, contacts=win._project.contacts * 3)
    win._history.push(too_many, "test")
    win.write_to_radio()
    assert warned and "上限" in warned[-1]
    assert struct.unpack_from("<H", too_many.image, 0x1393)[0] == 360  # untouched source image


def test_main_window_reads_and_writes_without_dialog(app: QApplication, radio: SimulatedRadio,
                                                     tmp_path: Path,
                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.main_window import MainWindow
    from opendrs.settings import load_settings

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    infos: list[str] = []
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: infos.append(a[2]))
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Yes)
    win = MainWindow()
    win._radio._sim = radio
    win._radio.port_box.setCurrentIndex(win._radio.port_box.findData(SIMULATED))
    busy: list[bool] = []
    win._radio.busy_changed.connect(busy.append)
    assert win.read_from_radio()
    assert not win._a_read.isEnabled() and not win.centralWidget().isEnabled()
    win._radio.wait()
    app.processEvents()
    app.processEvents()
    assert busy == [True, False] and win._a_read.isEnabled()
    assert win._tabs.tabText(1) == "頻道（27）" and win.is_dirty()
    assert load_settings().radio_port == SIMULATED  # remembered for next time
    edited = win._project.with_channel(0, win._project.channels[0].with_freq_a(145_500_000))
    win._history.push(edited, "edit")
    win.write_to_radio()
    win._radio.wait()
    app.processEvents()
    app.processEvents()
    assert radio.commits == 1 and any("寫入完成" in m for m in infos)
    assert RadioClient(radio).read_codeplug().project.channels[0].freq_a_hz == 145_500_000
    assert any("對講機時鐘已設為 " in m for m in infos)  # every write sets the radio's clock


def _without_date(stream: bytes) -> bytes:
    return stream[:0x60] + stream[0x70:]


def test_panel_clock_writes_the_radio_back_with_only_a_new_date(app: QApplication, radio: SimulatedRadio,
                                                                 tmp_path: Path) -> None:
    original = (CAP / "read_stream.bin").read_bytes()
    panel = make_panel(tmp_path, radio)
    [(outcome, error)] = run(panel, app, "clock")
    assert error == "" and outcome.mode == "clock" and radio.commits == 1
    assert "before_write" in outcome.backup.name
    back = RadioClient(radio).read_codeplug().stream
    assert _without_date(back) == _without_date(original) and back != original
    assert back[0x60:0x70].rstrip(b"\x00").decode() == outcome.date_text
    assert datetime.strptime(outcome.date_text, DATE_FORMAT)
    assert panel.status.text() == "校正時鐘完成"


def test_clock_refuses_data_it_cannot_rebuild(radio: SimulatedRadio) -> None:
    read = RadioClient(radio).read_codeplug()
    assert clock_write_project(read) == read.project
    odd = replace(read, stream=read.stream[:-1] + b"\x01")  # padding the parser drops
    with pytest.raises(RadioError, match="未寫入任何資料"):
        clock_write_project(odd)


def test_main_window_clock_keeps_the_open_project(app: QApplication, radio: SimulatedRadio, tmp_path: Path,
                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.main_window import MainWindow

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    infos: list[tuple[str, str]] = []
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: infos.append((a[1], a[2])))
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Yes)
    win = MainWindow()
    target = tmp_path / "mine.F136"
    target.write_bytes((CAP / "written_project.F136").read_bytes())
    assert win.open_path(target)
    mine = win._project
    win._radio._sim = radio
    win._radio.port_box.setCurrentIndex(win._radio.port_box.findData(SIMULATED))
    assert win.sync_radio_clock()
    assert not win._a_clock.isEnabled()
    win._radio.wait()
    app.processEvents()
    app.processEvents()
    assert win._a_clock.isEnabled() and win._project is mine and not win.is_dirty()
    [(title, text)] = infos
    assert title == "校正對講機時鐘" and "對講機時鐘已設為" in text and "沒有其他變更" in text
