"""Channel CSV export / import."""

from __future__ import annotations

import csv
import io
from pathlib import Path

import pytest

from opendrs import channel_fields as cf
from opendrs import channels_csv as cc
from opendrs import f136, layout
from opendrs.layout import zone_channel_rows
from opendrs.textcodec import NameCodec

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"
CODEC = NameCodec.BIG5


def _rows(data: bytes) -> list[list[str]]:
    return list(csv.reader(io.StringIO(data.decode("utf-8-sig"), newline="")))


def _csv(rows: list[list[str]], encoding: str = "utf-8-sig") -> bytes:
    buf = io.StringIO(newline="")
    csv.writer(buf, lineterminator="\r\n").writerows(rows)
    return buf.getvalue().encode(encoding)


def test_export_is_excel_friendly_and_round_trips() -> None:
    project = f136.load(DH98)
    data = cc.export_channels(project, CODEC)
    assert data.startswith(b"\xef\xbb\xbf") and b"\r\n" in data
    rows = _rows(data)
    assert tuple(rows[0]) == cc.HEADER
    assert rows[1][:5] == ["1", "MMDVM", "1", "TG46600", "439.85000"]
    assert rows[1][rows[0].index("發話對象")] == "2 TG46600_TW"  # reference: number and name
    result = cc.import_channels(project, data, CODEC)
    assert result.project == project and result.updated == result.added == 0 and not result.errors
    assert result.unchanged == len(rows) - 1


def test_update_cells_by_label_name_and_number() -> None:
    project = f136.load(DH98)
    rows = _rows(cc.export_channels(project, CODEC))
    head = rows[0]
    row = rows[4]  # zone 1, channel 4 (TN6A_relay)
    row[head.index("名稱")] = "TN6A_NEW"
    row[head.index("接收MHz")] = "431.6"
    row[head.index("功率")] = "低"
    row[head.index("時隙")] = "TS1"
    row[head.index("發話對象")] = "TG9"  # by exact contact name
    row[head.index("接收亞音")] = "88.5"
    row[head.index("發射亞音")] = "D023N"
    result = cc.import_channels(project, _csv([head, row]), CODEC)
    assert not result.errors and result.updated == 1 and result.added == 0
    ch = result.project.channels[zone_channel_rows(result.project, 0)[3]]
    assert ch.name(CODEC) == "TN6A_NEW" and ch.rx_hz == 431_600_000
    assert cf.get(ch, cf.POWER) == 0
    assert cf.slot_label(ch) == "時隙1" and ch.tx_contact == 13
    assert cf.tone_label(cf.tone(ch, False)) == "88.5 Hz" and cf.tone_label(cf.tone(ch, True)) == "D023N"
    # other channels untouched
    others = [i for i in range(len(project.channels)) if i != zone_channel_rows(project, 0)[3]]
    assert all(result.project.channels[i] == project.channels[i] for i in others)


def test_new_channel_appended_to_zone() -> None:
    project = f136.load(DH98)
    zone_len = len(zone_channel_rows(project, 1))
    rows = [["Zone", "Ch", "Name", "RX (MHz)", "TX (MHz)", "Mode"],  # English headers
            ["2", "", "NEW_ANALOG", "145.5", "145.5", "analog"]]
    result = cc.import_channels(project, _csv(rows), CODEC)
    assert not result.errors and result.added == 1
    new_rows = zone_channel_rows(result.project, 1)
    assert len(new_rows) == zone_len + 1
    ch = result.project.channels[new_rows[-1]]
    assert ch.name(CODEC) == "NEW_ANALOG" and ch.rx_hz == 145_500_000 and ch.mode == 0
    assert ch.index == zone_len + 1  # numbered within its zone
    assert len(result.project.channels) == len(project.channels) + 1


def test_bad_rows_are_skipped_whole_with_reasons() -> None:
    project = f136.load(DH98)
    head = list(cc.HEADER)
    good = _rows(cc.export_channels(project, CODEC))[2]
    good[head.index("名稱")] = "RENAMED"
    out_of_band = list(good)
    out_of_band[head.index("頻道")] = "1"
    out_of_band[head.index("名稱")] = "SHOULD_NOT_APPLY"
    out_of_band[head.index("接收MHz")] = "900"
    bad_zone = ["99", "", "1", "X"]
    bad_ref = list(good)
    bad_ref[head.index("頻道")] = "3"
    bad_ref[head.index("發話對象")] = "999"
    new_without_name = ["1", "", "", ""]
    result = cc.import_channels(project, _csv([head, good, out_of_band, bad_zone, bad_ref, new_without_name]), CODEC)
    assert result.updated == 1 and result.added == 0
    text = "\n".join(result.errors)
    assert "第 3 列「接收MHz」" in text and "超出本機頻段" in text
    assert "第 4 列：分區「99」不存在" in text and "第 5 列「發話對象」：編號 999 不存在" in text
    assert "第 6 列：新頻道需要名稱" in text
    first = result.project.channels[zone_channel_rows(result.project, 0)[0]]
    assert first.name(CODEC) != "SHOULD_NOT_APPLY"  # the whole row was skipped


def test_missing_columns_and_big5_file() -> None:
    project = f136.load(DH98)
    assert "缺少欄位" in cc.import_channels(project, _csv([["名稱"], ["x"]]), CODEC).errors[0]
    assert cc.import_channels(project, b"", CODEC).errors == ("檔案是空的",)
    rows = [["分區", "頻道", "名稱"], ["1", "1", "測試頻道"]]
    result = cc.import_channels(project, _csv(rows, "big5"), CODEC)
    assert result.encoding.lower().startswith("big5") and result.updated == 1
    assert result.project.channels[zone_channel_rows(result.project, 0)[0]].name(CODEC) == "測試頻道"


def test_menu_export_import_and_undo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import os

    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtWidgets import QApplication

    QApplication.instance() or QApplication([])
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    from opendrs.gui.main_window import MainWindow

    project_path = tmp_path / "p.F136"
    project_path.write_bytes(DH98.read_bytes())
    win = MainWindow()
    assert win.open_path(project_path)
    csv_path = tmp_path / "ch.csv"
    assert win.export_channels_csv(csv_path)
    rows = _rows(csv_path.read_bytes())
    rows[1][rows[0].index("名稱")] = "FROM_CSV"
    csv_path.write_bytes(_csv(rows))
    before = win._project
    result = win.import_channels_csv(csv_path, confirm=False)
    assert result is not None and result.updated == 1 and win.is_dirty()
    assert win._project.channels[zone_channel_rows(win._project, 0)[0]].name(CODEC) == "FROM_CSV"
    win.undo()
    assert win._project == before
    win.close()


def _names(project: f136.Project, zone_row: int) -> list[str]:
    return [project.channels[r].name(CODEC) for r in zone_channel_rows(project, zone_row)]


def test_positions_refer_to_the_zone_before_the_import() -> None:
    project = f136.load(DH98)
    assert len(zone_channel_rows(project, 0)) == 7
    rows = [["分區", "頻道", "名稱"], ["1", "9", "SECOND"], ["1", "8", "FIRST"]]
    result = cc.import_channels(project, _csv(rows), CODEC)
    assert not result.errors and (result.added, result.updated) == (2, 0)
    assert _names(result.project, 0)[7:] == ["SECOND", "FIRST"]  # appended in file order


def test_same_channel_twice_is_rejected() -> None:
    rows = [["分區", "頻道", "名稱"], ["1", "1", "AAA"], ["1", "1", "BBB"]]
    result = cc.import_channels(f136.load(DH98), _csv(rows), CODEC)
    assert result.updated == 1 and _names(result.project, 0)[0] == "AAA"
    assert result.errors == ("第 3 列：分區 1 第 1 台已由第 2 列修改，同一頻道只能出現一次",)


def test_reference_number_and_name_must_agree() -> None:
    project = f136.load(DH98)
    row13 = next(i for i, c in enumerate(project.contacts) if c.index == 13)
    project = project.with_contact(row13, project.contacts[row13].with_name("5 Stars", CODEC))
    head = ["分區", "頻道", "發話對象"]
    result = cc.import_channels(project, _csv([head, ["1", "1", "13 WRONG_NAME"]]), CODEC)
    assert result.errors == ("第 2 列「發話對象」：編號 13 是「5 Stars」，不是「WRONG_NAME」",)
    result = cc.import_channels(project, _csv([head, ["1", "1", "5 Stars"], ["1", "2", "2 TG46600_TW"]]), CODEC)
    assert not result.errors
    first, second = (result.project.channels[r] for r in zone_channel_rows(result.project, 0)[:2])
    assert (first.tx_contact, second.tx_contact) == (13, 2)  # by exact name, then number + name


@pytest.mark.parametrize(("column", "value", "message"), [
    ("接收MHz", "inf", "無效頻率"), ("接收MHz", "nan", "無效頻率"), ("接收MHz", "0", "頻率不可為 0"),
    ("接收亞音", "snan", "無效的亞音"), ("頻道", "²", "不是數字"), ("分區", "²", "不存在"),
])
def test_odd_cells_skip_the_row_instead_of_crashing(column: str, value: str, message: str) -> None:
    head = ["分區", "頻道", "名稱", "接收MHz", "接收亞音"]
    row = ["1", "1", "X", "439.5", "88.5"]
    row[head.index(column)] = value
    result = cc.import_channels(f136.load(DH98), _csv([head, row]), CODEC)
    assert result.updated == 0 and len(result.errors) == 1 and message in result.errors[0]


def test_unparseable_csv_and_broken_layout_are_reported() -> None:
    project = f136.load(DH98)
    huge = _csv([["分區", "頻道", "名稱"], ["1", "1", "x" * 200_000]])
    assert cc.import_channels(project, huge, CODEC).errors[0].startswith("無法解析 CSV")
    zone = project.zones[0]
    broken = project.with_zone(0, zone.with_layout(zone.first_record, zone.channel_count + 1))
    assert "分區配置有誤" in cc.import_channels(broken, _csv([["分區", "頻道"], ["1", "1"]]), CODEC).errors[0]
    with pytest.raises(layout.LayoutError):
        cc.export_channels(broken, CODEC)


def test_zone_name_must_match_the_zone_number() -> None:
    rows = [["分區", "分區名稱", "頻道", "名稱"], ["4", "MMDVM", "1", "MOVED"], ["1", "MMDVM", "1", "OK"]]
    result = cc.import_channels(f136.load(DH98), _csv(rows), CODEC)
    assert result.updated == 1 and len(result.errors) == 1
    assert "分區名稱「MMDVM」與分區 4" in result.errors[0] and _names(result.project, 0)[0] == "OK"


def test_reimporting_the_same_file_does_not_add_twice() -> None:
    data = _csv([["分區", "頻道", "名稱", "接收MHz", "發射MHz"], ["2", "", "NEW", "145.5", "145.5"]])
    once = cc.import_channels(f136.load(DH98), data, CODEC)
    twice = cc.import_channels(once.project, data, CODEC)
    assert once.added == 1 and twice.added == 0 and twice.project == once.project
    assert "已有相同名稱與頻率的頻道" in twice.errors[0]


def test_semicolon_files_and_mhz_suffix() -> None:
    data = "分區;頻道;接收MHz\r\n1;1;439.5 MHz\r\n".encode("utf-8-sig")
    result = cc.import_channels(f136.load(DH98), data, CODEC)
    assert not result.errors and result.updated == 1
    assert result.project.channels[zone_channel_rows(result.project, 0)[0]].rx_hz == 439_500_000
