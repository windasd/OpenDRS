"""「讀取錄音」dialog against the simulated radio."""

from __future__ import annotations

import os
import wave
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from opendrs.gui.recording_dialog import (  # noqa: E402
    RadioIdNames,
    RecordingDialog,
    duration_text,
)
from opendrs.recording import DEFAULT_AMBEP2, RecordInfo  # noqa: E402
from opendrs.recording_sim import SimulatedRecorder  # noqa: E402


@pytest.fixture
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def _run(app: QApplication, dialog: RecordingDialog, start) -> None:  # noqa: ANN001
    assert start()
    dialog.wait()
    app.processEvents()


def test_list_then_download_checked(app: QApplication, tmp_path: Path) -> None:
    radio = SimulatedRecorder()
    dialog = RecordingDialog(lambda: radio, "模擬對講機", tmp_path / "rec", tmp_path / "missing.exe",
                             names={9998765: "QA0AJS"}, pause=0)
    _run(app, dialog, dialog.read_list)
    assert dialog.table.rowCount() == 3 and dialog.error == ""
    assert dialog.table.item(0, 3).text() == "QA0AJS" and dialog.table.item(1, 1).text() == "發射"
    assert "18.7 秒" == dialog.table.item(0, 4).text()
    dialog.table.item(1, 0).setCheckState(Qt.CheckState.Unchecked)
    assert [i.number for i in dialog.checked()] == [1, 3]
    _run(app, dialog, dialog.download_checked)
    assert [s.info.number for s in dialog.saved] == [1, 3]
    raw = tmp_path / "rec" / "20250607_1234_1_R09998765.ambe"
    assert raw.read_bytes() == radio.records[0][1]
    assert "找不到 ambep2" in dialog.table.item(0, 6).text()  # no decoder: raw data kept
    assert not dialog.play(0) and "還沒有下載成 WAV" in dialog.status.text()


def test_download_all_without_list_support(app: QApplication, tmp_path: Path) -> None:
    radio = SimulatedRecorder(supports_list=False)
    dialog = RecordingDialog(lambda: radio, "COM3", tmp_path, tmp_path / "missing.exe", pause=0)
    _run(app, dialog, dialog.read_list)
    assert "不支援從電腦讀取錄音" in dialog.status.text() and dialog.table.rowCount() == 0
    _run(app, dialog, dialog.download_all)
    assert [s.info.number for s in dialog.saved] == [1, 2, 3] and dialog.table.rowCount() == 3
    assert "已下載 3 筆" in dialog.status.text()


def test_no_port_and_nothing_checked(app: QApplication, tmp_path: Path) -> None:
    dialog = RecordingDialog(None, "", tmp_path, tmp_path / "x.exe")
    assert not dialog.list_btn.isEnabled() and not dialog.read_list()
    assert "選擇寫頻線" in dialog.status.text()
    other = RecordingDialog(lambda: SimulatedRecorder(), "COM3", tmp_path, tmp_path / "x.exe", pause=0)
    assert not other.download_checked() and "讀取清單" in other.status.text()


@pytest.mark.skipif(not DEFAULT_AMBEP2.is_file(), reason="vendor ambep2.exe not installed")
def test_download_decodes_to_wav(app: QApplication, tmp_path: Path) -> None:
    radio = SimulatedRecorder()
    dialog = RecordingDialog(lambda: radio, "模擬對講機", tmp_path, DEFAULT_AMBEP2, pause=0)
    _run(app, dialog, dialog.read_list)
    for row in (1, 2):
        dialog.table.item(row, 0).setCheckState(Qt.CheckState.Unchecked)
    _run(app, dialog, dialog.download_checked)
    wav_path = dialog.saved[0].wav
    assert wav_path is not None and wav_path.name == "20250607_1234_1_R09998765.wav"
    with wave.open(str(wav_path), "rb") as w:
        assert w.getframerate() == 8000 and w.getnframes() == 8433 // 9 * 160


def test_status_counts_only_this_run_and_names_do_not_collide(app: QApplication, tmp_path: Path) -> None:
    radio = SimulatedRecorder()
    dialog = RecordingDialog(lambda: radio, "模擬對講機", tmp_path, tmp_path / "missing.exe", pause=0)
    _run(app, dialog, dialog.read_list)
    for row in (1, 2):
        dialog.table.item(row, 0).setCheckState(Qt.CheckState.Unchecked)
    _run(app, dialog, dialog.download_checked)
    _run(app, dialog, dialog.read_list)
    assert "對講機回報 3 筆" in dialog.status.text()  # not overwritten by the earlier download count
    for row in (1, 2):
        dialog.table.item(row, 0).setCheckState(Qt.CheckState.Unchecked)
    # the radio now holds a different call under record 1 (same time, direction and ID)
    info, data = radio.records[0]
    radio.records[0] = (info, bytes(reversed(data)))
    _run(app, dialog, dialog.download_checked)
    assert "已下載 1 筆" in dialog.status.text()
    names = sorted(p.name for p in tmp_path.glob("*.ambe"))
    assert names == ["20250607_1234_1_R09998765.ambe", "20250607_1234_1_R09998765_2.ambe"]


def test_stop_and_close_while_listing(app: QApplication, tmp_path: Path) -> None:
    import time

    class Quiet:  # opens fine, never answers
        def write(self, data: bytes) -> None:
            pass

        def read_idle(self, idle: float, max_wait: float) -> bytes:
            time.sleep(0.02)
            return b""

        def reset_input(self) -> None:
            pass

        def close(self) -> None:
            pass

    dialog = RecordingDialog(Quiet, "COM3", tmp_path, tmp_path / "x.exe", pause=0)
    dialog.show()
    assert dialog.read_list() and not dialog.read_list()  # one transfer at a time
    dialog.reject()
    assert dialog.isVisible() and "傳輸進行中" in dialog.status.text()
    dialog.stop.click()
    dialog.wait(5000)
    app.processEvents()
    assert dialog.status.text() == "已停止。" and not dialog.stop.isVisible()
    dialog.out_dir.setText("  ")
    assert not dialog.read_list() and "錄音資料夾" in dialog.status.text()
    dialog.reject()
    assert not dialog.isVisible()


def _order(dialog: RecordingDialog) -> list[str]:
    """Record numbers as the rows currently show them."""
    return [dialog.table.item(row, 0).text() for row in range(dialog.table.rowCount())]


def test_lookup_names_ids_missing_from_project_contacts(app: QApplication, tmp_path: Path) -> None:
    calls: list[int] = []

    def lookup(call_id: int) -> str:
        calls.append(call_id)
        return {9998765: "QA0AX 不該出現", 12345: "QA0ABT Zhi Ming"}.get(call_id, "")

    dialog = RecordingDialog(lambda: SimulatedRecorder(), "模擬對講機", tmp_path, tmp_path / "x.exe",
                             names={9998765: "QA0AJS"}, lookup=lookup, pause=0)
    _run(app, dialog, dialog.read_list)
    assert [dialog.table.item(row, 3).text() for row in range(3)] == ["QA0AJS", "QA0ABT Zhi Ming", ""]
    assert dialog.table.item(0, 3).toolTip() == "" and "RadioID" in dialog.table.item(1, 3).toolTip()
    _run(app, dialog, dialog.read_list)
    assert calls == [12345, 9]  # project contacts win; each other ID is looked up once


def test_radioid_names_from_downloaded_cache(tmp_path: Path) -> None:
    assert RadioIdNames.from_cache(tmp_path) is None  # not downloaded: no names, no error
    (tmp_path / "user.csv").write_bytes(
        b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n"
        b"19990186,QA0AKN,Wrong,,,,\r\n"
        b"9990186,QA0ABT,Zhi Ming,,New Taipei City,Taipei,Taiwan\r\n"
        + "9991032,QA0AU,小美,林,New Taipei City,Taipei,Taiwan".encode())  # last line, no newline
    names = RadioIdNames.from_cache(tmp_path)
    assert names is not None and names(9990186) == "QA0ABT Zhi Ming"
    (tmp_path / "user.csv").unlink()  # read once, then searched in memory
    assert names(9991032) == "QA0AU 小美 林"
    assert names(46621) == "" and names(1) == ""  # a talkgroup, the all-call ID


def test_select_all_none_and_sorting_keep_checked_records(app: QApplication, tmp_path: Path) -> None:
    radio = SimulatedRecorder()
    dialog = RecordingDialog(lambda: radio, "模擬對講機", tmp_path, tmp_path / "missing.exe", pause=0)
    _run(app, dialog, dialog.read_list)
    assert dialog.table.isSortingEnabled() and _order(dialog) == ["1", "2", "3"]  # list order at first
    dialog.select_none.click()
    assert dialog.checked() == []
    dialog.select_all.click()
    assert [i.number for i in dialog.checked()] == [1, 2, 3]
    header = dialog.table.horizontalHeader()
    header.setSortIndicator(4, Qt.SortOrder.AscendingOrder)  # what clicking 長度 does
    assert _order(dialog) == ["2", "3", "1"]  # 6.7 s, 9.0 s, 18.7 s (as text, "18.7" would come first)
    dialog.table.item(0, 0).setCheckState(Qt.CheckState.Unchecked)  # record 2, now the top row
    assert [i.number for i in dialog.checked()] == [1, 3] and dialog.row_of(1) == 2
    _run(app, dialog, dialog.download_checked)
    assert [s.info.number for s in dialog.saved] == [1, 3] and _order(dialog) == ["2", "3", "1"]
    assert dialog.table.item(2, 6).text().startswith("20250607_1234_1_R09998765.ambe")
    assert dialog.table.item(0, 6) is None and dialog.table.item(0, 0).checkState() == Qt.CheckState.Unchecked
    header.setSortIndicator(2, Qt.SortOrder.AscendingOrder)  # 對方 ID as numbers: 9, 12345, 9998765
    assert _order(dialog) == ["3", "2", "1"]
    header.setSortIndicator(4, Qt.SortOrder.DescendingOrder)
    _run(app, dialog, dialog.read_list)  # a new list keeps the chosen sort
    assert _order(dialog) == ["1", "3", "2"] and dialog.row_of(9) == -1
    header.setSortIndicator(1, Qt.SortOrder.DescendingOrder)  # a text column (no numeric key)
    assert [dialog.table.item(row, 1).text() for row in range(3)] == ["發射", "接收", "接收"]


def test_total_line(app: QApplication, tmp_path: Path) -> None:
    assert (duration_text(34.4), duration_text(192), duration_text(3725)) == ("34 秒", "3 分 12 秒",
                                                                             "1 小時 2 分 5 秒")
    radio = SimulatedRecorder()
    dialog = RecordingDialog(lambda: radio, "模擬對講機", tmp_path, tmp_path / "missing.exe", pause=0)
    assert dialog.summary.text() == ""
    _run(app, dialog, dialog.read_list)
    assert dialog.summary.text() == "共 3 筆，合計 34 秒"  # 18.7 + 6.7 + 9.0 s
    other = RecordingDialog(lambda: radio, "模擬對講機", tmp_path, tmp_path / "missing.exe", pause=0)
    _run(app, other, other.download_all)
    assert other.summary.text() == "共 3 筆，合計 34 秒" and "合計 34 秒" in other.status.text()
    long_calls = SimulatedRecorder(records=[(RecordInfo(1, 2, "R", 46621, 27000, "2026/10/09 10:00"), b""),
                                            (RecordInfo(2, 2, "S", 1, 26550, "2026/10/09 10:05"), b"")])
    third = RecordingDialog(lambda: long_calls, "模擬對講機", tmp_path, tmp_path / "missing.exe", pause=0)
    _run(app, third, third.read_list)
    assert third.summary.text() == "共 2 筆，合計 1 分 59 秒"  # 60 s + 59 s


def test_settings_round_trip(tmp_path: Path) -> None:
    from opendrs.settings import Settings, load_settings, save_settings

    save_settings(Settings(ambep2_path="D:/tools/ambep2.exe", recordings_dir="D:/rec"), tmp_path)
    loaded = load_settings(tmp_path)
    assert (loaded.ambep2_path, loaded.recordings_dir) == ("D:/tools/ambep2.exe", "D:/rec")
