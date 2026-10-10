"""Render the application icon for the Rust build from the Python version's drawing.

The icon is the one `gui/theme.py::app_icon` draws (the radio on an accent-coloured rounded
square), rendered straight from its SVG at every size so no size is scaled from another:

- `opendrs-gui/assets/opendrs.png`  256×256, the window icon and the Linux desktop icon;
- `opendrs-gui/assets/opendrs.ico`  16–256, embedded in the Windows .exe;
- `opendrs-gui/assets/opendrs.icns` 16–1024, the macOS app bundle's icon;

and every stroke icon of `theme._ICONS` (toolbar, problem list) as
`opendrs-gui/assets/icons/<name>.svg`, drawn as `theme.icon` draws it but in white, so the egui
version can tint it with the theme's colour.

Run from the repository root with the Python version on PYTHONPATH (needs PySide6):
    PYTHONPATH=../opendrs-oracle/src python rust/tools/make_icons.py
"""

from __future__ import annotations

import os
import struct
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QBuffer, QByteArray, QIODevice, Qt  # noqa: E402
from PySide6.QtGui import QGuiApplication, QImage, QPainter  # noqa: E402
from PySide6.QtSvg import QSvgRenderer  # noqa: E402

from opendrs.gui.theme import _ICONS, LIGHT  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "rust" / "opendrs-gui" / "assets"
ICONS = ASSETS / "icons"
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
# (type, pixels): the PNG-based entries of the icns format.
ICNS_TYPES = ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256),
              (b"ic09", 512), (b"ic10", 1024))


def svg() -> bytes:
    """The SVG `theme.app_icon` renders."""
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
            f'<rect width="24" height="24" rx="5" fill="{LIGHT.accent}"/>'
            '<g fill="none" stroke="#ffffff" stroke-width="1.6" stroke-linecap="round" '
            'stroke-linejoin="round" transform="translate(2 2) scale(0.83)">'
            + _ICONS["radio"] + "</g></svg>").encode()


def stroke_icon(name: str) -> str:
    """One of `theme._ICONS` as `theme.icon` draws it, in white."""
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" '
            'fill="none" stroke="#ffffff" stroke-width="1.8" stroke-linecap="round" '
            f'stroke-linejoin="round">{_ICONS[name]}</svg>\n')


def png(renderer: QSvgRenderer, size: int) -> bytes:
    image = QImage(size, size, QImage.Format.Format_ARGB32)
    image.fill(Qt.GlobalColor.transparent)
    painter = QPainter(image)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    renderer.render(painter)
    painter.end()
    raw = QByteArray()
    buffer = QBuffer(raw)
    buffer.open(QIODevice.OpenModeFlag.WriteOnly)
    image.save(buffer, "PNG")
    return bytes(raw.data())


def ico(pngs: dict[int, bytes]) -> bytes:
    """A multi-size .ico whose entries are PNG images (as packaging/build.py writes it)."""
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = len(header) + 16 * len(pngs)
    entries, data = b"", b""
    for size, image in sorted(pngs.items()):
        side = 0 if size >= 256 else size  # 0 means 256
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(image), offset + len(data))
        data += image
    return header + entries + data


def icns(renderer: QSvgRenderer) -> bytes:
    body = b""
    for kind, size in ICNS_TYPES:
        image = png(renderer, size)
        body += kind + struct.pack(">I", 8 + len(image)) + image
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def main() -> None:
    app = QGuiApplication.instance() or QGuiApplication([])  # noqa: F841 - QPainter needs it
    renderer = QSvgRenderer(QByteArray(svg()))
    ASSETS.mkdir(parents=True, exist_ok=True)
    (ASSETS / "opendrs.png").write_bytes(png(renderer, 256))
    (ASSETS / "opendrs.ico").write_bytes(ico({size: png(renderer, size) for size in ICO_SIZES}))
    (ASSETS / "opendrs.icns").write_bytes(icns(renderer))
    for name in ("opendrs.png", "opendrs.ico", "opendrs.icns"):
        print(f"{ASSETS / name}  {(ASSETS / name).stat().st_size:,} bytes")
    ICONS.mkdir(exist_ok=True)
    for old in ICONS.glob("*.svg"):
        old.unlink()
    for name in sorted(_ICONS):
        (ICONS / f"{name}.svg").write_text(stroke_icon(name), encoding="utf-8", newline="\n")
    print(f"{ICONS}  {len(_ICONS)} 個圖示")


if __name__ == "__main__":
    main()
