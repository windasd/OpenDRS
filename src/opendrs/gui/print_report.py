"""Printable project report (the legacy CPS 列印): device info, channels by zone,
contacts, RX groups and scan lists as one HTML document, printed or saved as PDF."""

from __future__ import annotations

from datetime import datetime
from html import escape
from pathlib import Path
from typing import TypeVar

from PySide6.QtCore import QMarginsF
from PySide6.QtGui import (
    QPagedPaintDevice,
    QPageLayout,
    QPageSize,
    QPdfWriter,
    QTextDocument,
)

from .. import __version__, hardware
from ..channels_csv import FIELDS
from ..f136 import Project
from ..layout import zone_channel_rows
from ..rxgroup import has_rx_groups, rx_groups
from ..scanlist import TX_MODES, has_scan_lists, scan_lists
from ..textcodec import NameCodec
from .scan_page import entry_label

Paged = TypeVar("Paged", bound=QPagedPaintDevice)
SECTIONS = (("device", "裝置資訊"), ("channels", "頻道"), ("contacts", "聯絡人"),
            ("groups", "接收群組"), ("scans", "掃描清單"))
CHANNEL_COLUMNS = ("頻道", "名稱", "接收MHz", "發射MHz", "模式", "色碼", "時隙", "功率",
                   "發話對象", "接收群組", "掃描列表", "接收亞音", "發射亞音")
PAGE_MARGIN_MM = 0  # QTextDocument.print_ adds its own 2 cm margin inside the page
STYLE = """
body { font-size: 8pt; }
h1 { font-size: 14pt; margin: 0; }
h2 { font-size: 11pt; margin: 10px 0 4px 0; }
th.caption { background: #ffffff; border: none; font-size: 9pt; padding: 8px 0 2px 0; }
table { border-collapse: collapse; }
th { background: #e8e8e8; text-align: left; }
th, td { border: 1px solid #999; padding: 1px 4px; }
.meta { color: #555; }
"""


def _table(header: tuple[str, ...] | list[str], rows: list[list[str]], caption: str = "") -> str:
    """A table whose header rows (caption included) repeat on every printed page, so a
    caption is never left alone at the bottom of a page."""
    head = "".join(f"<th>{escape(h)}</th>" for h in header)
    if caption:
        head = f'<th class="caption" colspan="{len(header)}">{escape(caption)}</th></tr><tr>{head}'
    body = "".join("<tr>" + "".join(f"<td>{escape(c)}</td>" for c in row) + "</tr>" for row in rows)
    return f'<table width="100%" cellspacing="0" cellpadding="2"><thead><tr>{head}</tr></thead>{body}</table>'


def _device(project: Project) -> str:
    dev = project.device
    also = hardware.known_as(dev.model)
    rows = [["機型", f"{dev.model}（{also}）" if also else dev.model], ["型號", dev.model_number],
            ["機名", dev.alias], ["DMR ID", "" if dev.dmr_id is None else str(dev.dmr_id)],
            ["韌體", f"{dev.firmware} {dev.firmware_date}".strip()], ["頻段", dev.band_label],
            ["寫入時間", dev.write_date], ["出廠編號", dev.factory_number], ["序號", dev.serial_number]]
    return _table(("項目", "內容"), [r for r in rows if r[1]])


def _channels(project: Project, codec: NameCodec) -> str:
    fields = {f.header: f for f in FIELDS}
    parts = []
    for zone_row, zone in enumerate(project.zones):
        rows = []
        for position, row in enumerate(zone_channel_rows(project, zone_row), start=1):
            ch = project.channels[row]
            rows.append([str(position)] + [fields[h].get(project, ch, codec) for h in CHANNEL_COLUMNS[1:]])
        parts.append(_table(CHANNEL_COLUMNS, rows,
                            f"分區 {zone_row + 1}：{zone.name(codec)}（{len(rows)} 個頻道）"))
    return "".join(parts)


def _contacts(project: Project, codec: NameCodec) -> str:
    rows = [[str(c.index), c.name(codec), c.kind_label, str(c.call_id)] for c in project.contacts]
    return _table(("編號", "名稱", "類型", "ID"), rows)


def _groups(project: Project, codec: NameCodec) -> str:
    names = {c.index: c.name(codec) for c in project.contacts}
    rows = [[str(g.number), g.name(codec), "、".join(names.get(m, f"#{m}") for m in g.members)]
            for g in rx_groups(project.image) if g.in_use]
    return _table(("編號", "名稱", "成員"), rows)


def _scans(project: Project, codec: NameCodec) -> str:
    modes = {value: label for label, value in TX_MODES}
    rows = []
    for scan in scan_lists(project.image):
        if not scan.in_use:
            continue
        mode = modes.get(scan.tx_mode, str(scan.tx_mode))
        if scan.tx_mode == 2 and scan.designated != (0, 0):
            mode += f"：{entry_label(project, scan.designated, codec)}"
        rows.append([str(scan.number), scan.name(codec), "開" if scan.talkback else "關", mode,
                     "、".join(entry_label(project, e, codec) for e in scan.entries)])
    return _table(("編號", "名稱", "回講", "發射模式", "成員"), rows)


def report_html(project: Project, codec: NameCodec, title: str = "",
                sections: tuple[str, ...] = tuple(k for k, _ in SECTIONS),
                now: datetime | None = None) -> str:
    """The whole report as HTML (Qt rich-text subset)."""
    stamp = (now or datetime.now().astimezone()).strftime("%Y-%m-%d %H:%M")
    out = [f"<html><head><meta charset='utf-8'><style>{STYLE}</style></head><body>",
           f"<h1>{escape(title or '寫頻資料')}</h1>",
           f"<p class='meta'>列印時間 {stamp}　·　OpenDRS{__version__}</p>"]
    builders = {"device": lambda: _device(project), "channels": lambda: _channels(project, codec),
                "contacts": lambda: _contacts(project, codec),
                "groups": lambda: _groups(project, codec) if has_rx_groups(project.image) else "",
                "scans": lambda: _scans(project, codec) if has_scan_lists(project.image) else ""}
    for key, heading in SECTIONS:
        if key in sections:
            body = builders[key]()
            if body:
                out.append(f"<h2>{heading}</h2>{body}")
    out.append("</body></html>")
    return "".join(out)


def document(html: str) -> QTextDocument:
    doc = QTextDocument()
    doc.setHtml(html)
    return doc


def setup_printer(printer: Paged) -> Paged:
    """A4 landscape (the channel table is wide), small margins."""
    printer.setPageLayout(QPageLayout(QPageSize(QPageSize.PageSizeId.A4), QPageLayout.Orientation.Landscape,
                                      QMarginsF(*[PAGE_MARGIN_MM] * 4), QPageLayout.Unit.Millimeter))
    return printer


def write_pdf(html: str, path: Path) -> None:
    before = path.stat().st_mtime_ns if path.is_file() else None
    writer = setup_printer(QPdfWriter(str(path)))  # not QPrinter: no print-spooler queries
    writer.setTitle(path.stem)
    writer.setCreator(f"OpenDRS{__version__}")
    document(html).print_(writer)
    del writer  # flushes and closes the file
    # QPdfWriter reports no errors; check the file instead
    if not path.is_file() or path.stat().st_size == 0 or path.stat().st_mtime_ns == before:
        raise OSError(f"無法寫入 {path}（檔案是否已在其他程式開啟？）")
