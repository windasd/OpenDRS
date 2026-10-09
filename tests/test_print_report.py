"""Printable report (列印／預覽列印／匯出 PDF)."""

from __future__ import annotations

import os
from datetime import UTC, datetime
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PySide6.QtWidgets import QApplication

from opendrs import f136
from opendrs.gui import print_report as pr
from opendrs.textcodec import NameCodec

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"
CODEC = NameCodec.BIG5


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def test_report_lists_every_section_and_channel(app: QApplication) -> None:
    project = f136.load(DH98)
    html = pr.report_html(project, CODEC, "dh98vu", now=datetime(2026, 10, 9, 12, 0, tzinfo=UTC))
    assert "<h1>dh98vu</h1>" in html and "列印時間 2026-10-09 12:00" in html
    for _, heading in pr.SECTIONS:
        assert f"<h2>{heading}</h2>" in html
    assert "DRS-8500（Kydera DR-8500UV／HORA DH-98VU）" in html
    assert '<th class="caption" colspan="13">分區 1：MMDVM（7 個頻道）</th>' in html
    assert html.count("<thead>") == len(project.zones) + 4  # device, zones, contacts, groups, scans
    assert "<td>TN6A_relay</td><td>431.58000</td><td>439.58000</td>" in html
    assert "<td>10 TG46621_TW</td>" in html  # reference as number + name
    for contact in project.contacts:
        assert f"<td>{contact.call_id}</td>" in html


def test_sections_can_be_chosen_and_text_is_escaped(app: QApplication) -> None:
    project = f136.load(DH98)
    first = project.channels[0].with_name("A<B&C", CODEC)
    project = project.with_channel(0, first)
    html = pr.report_html(project, CODEC, "x<y>", sections=("channels",))
    assert "<h1>x&lt;y&gt;</h1>" in html and "<td>A&lt;B&amp;C</td>" in html
    assert "<h2>頻道</h2>" in html and "<h2>聯絡人</h2>" not in html and "<h2>裝置資訊</h2>" not in html


def test_write_pdf(app: QApplication, tmp_path: Path) -> None:
    out = tmp_path / "report.pdf"
    pr.write_pdf(pr.report_html(f136.load(DH98), CODEC, "t"), out)
    assert out.read_bytes().startswith(b"%PDF-") and out.stat().st_size > 10_000
    with pytest.raises(OSError, match="無法寫入"):
        pr.write_pdf("<p>x</p>", tmp_path / "missing" / "a.pdf")


def test_menu_actions(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    from PySide6.QtWidgets import QMessageBox

    from opendrs.gui.main_window import MainWindow

    told: list[str] = []
    monkeypatch.setattr(QMessageBox, "information", lambda parent, title, text: told.append(title))
    win = MainWindow()
    assert not win.export_pdf(tmp_path / "none.pdf") and told == ["匯出 PDF"]  # no project yet
    target = tmp_path / "radio.F136"
    target.write_bytes(DH98.read_bytes())
    assert win.open_path(target)
    assert "<h1>radio</h1>" in win.report_html()
    assert win.export_pdf(tmp_path / "radio.pdf")
    assert (tmp_path / "radio.pdf").read_bytes().startswith(b"%PDF-")
    preview = win.print_preview()
    assert preview is not None and preview.isVisible()
    preview.close()
    labels = [a.text() for m in win.menuBar().actions() for a in m.menu().actions()]
    assert {"列印…", "預覽列印…", "匯出 PDF…"} <= set(labels)
    win.close()
