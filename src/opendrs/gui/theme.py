"""Look and feel: Fusion style, a light and a dark palette, one style sheet, icons.

Colours are defined once per theme (``Tokens``) and the style sheet is generated from
them, so both themes stay consistent. Icons are small stroke drawings in SVG, tinted
with the theme's text colour at render time, so they need no image files.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from PySide6.QtCore import QByteArray, QRectF, QSize, Qt
from PySide6.QtGui import QColor, QFont, QIcon, QPainter, QPalette, QPixmap
from PySide6.QtSvg import QSvgRenderer
from PySide6.QtWidgets import QApplication


class ThemeMode(str, Enum):
    AUTO = "auto"
    LIGHT = "light"
    DARK = "dark"

    @property
    def label(self) -> str:
        return {"auto": "跟隨系統", "light": "淺色", "dark": "深色"}[self.value]


@dataclass(frozen=True)
class Tokens:
    window: str
    surface: str  # cards, tables, inputs
    surface_alt: str  # alternating rows, headers
    border: str
    text: str
    muted: str
    accent: str
    accent_text: str
    accent_soft: str  # selection / hover background
    danger: str
    ok: str


LIGHT = Tokens(window="#f3f5f8", surface="#ffffff", surface_alt="#f7f8fa", border="#d8dde5",
               text="#1f2933", muted="#6b7684", accent="#2563eb", accent_text="#ffffff",
               accent_soft="#dbe7fe", danger="#c62828", ok="#2e7d32")
DARK = Tokens(window="#1b1f24", surface="#23282f", surface_alt="#2a3038", border="#3a414b",
              text="#e6e9ee", muted="#9aa4b1", accent="#5b8def", accent_text="#ffffff",
              accent_soft="#2c3d5f", danger="#ef6b6b", ok="#66bb6a")

_current = LIGHT


def tokens() -> Tokens:
    return _current


def system_is_dark(app: QApplication) -> bool:
    hints = app.styleHints()
    scheme = getattr(hints, "colorScheme", None)
    return bool(scheme and scheme() == Qt.ColorScheme.Dark)


def resolve(mode: ThemeMode, app: QApplication) -> Tokens:
    if mode is ThemeMode.AUTO:
        return DARK if system_is_dark(app) else LIGHT
    return DARK if mode is ThemeMode.DARK else LIGHT


def palette(t: Tokens) -> QPalette:
    p = QPalette()
    roles = {
        QPalette.ColorRole.Window: t.window, QPalette.ColorRole.WindowText: t.text,
        QPalette.ColorRole.Base: t.surface, QPalette.ColorRole.AlternateBase: t.surface_alt,
        QPalette.ColorRole.Text: t.text, QPalette.ColorRole.Button: t.surface,
        QPalette.ColorRole.ButtonText: t.text, QPalette.ColorRole.Highlight: t.accent,
        QPalette.ColorRole.HighlightedText: t.accent_text, QPalette.ColorRole.ToolTipBase: t.surface,
        QPalette.ColorRole.ToolTipText: t.text, QPalette.ColorRole.PlaceholderText: t.muted,
        QPalette.ColorRole.Link: t.accent,
    }
    for role, colour in roles.items():
        p.setColor(role, QColor(colour))
    for role in (QPalette.ColorRole.Text, QPalette.ColorRole.WindowText, QPalette.ColorRole.ButtonText):
        p.setColor(QPalette.ColorGroup.Disabled, role, QColor(t.muted))
    return p


def style_sheet(t: Tokens) -> str:
    return f"""
QMainWindow, QDialog {{ background: {t.window}; }}
QToolTip {{ background: {t.surface}; color: {t.text}; border: 1px solid {t.border}; padding: 4px 6px; }}

QMenuBar {{ background: {t.window}; padding: 2px 4px; }}
QMenuBar::item {{ padding: 4px 10px; border-radius: 4px; }}
QMenuBar::item:selected {{ background: {t.accent_soft}; }}
QMenu {{ background: {t.surface}; border: 1px solid {t.border}; padding: 4px; }}
QMenu::item {{ padding: 5px 24px 5px 10px; border-radius: 4px; }}
QMenu::item:selected {{ background: {t.accent_soft}; color: {t.text}; }}
QMenu::separator {{ height: 1px; background: {t.border}; margin: 4px 6px; }}

QToolBar {{ background: {t.surface}; border: none; border-bottom: 1px solid {t.border};
            padding: 4px 6px; spacing: 4px; }}
QToolBar::separator {{ width: 1px; background: {t.border}; margin: 4px 6px; }}
QToolButton {{ padding: 5px 8px; border-radius: 6px; border: 1px solid transparent; }}
QToolButton:hover {{ background: {t.accent_soft}; }}
QToolButton:pressed {{ background: {t.border}; }}
QToolButton:disabled {{ color: {t.muted}; }}
QToolButton#primary {{ background: {t.accent}; color: {t.accent_text}; }}
QToolButton#primary:hover {{ background: {t.accent}; border-color: {t.text}; }}

QTabWidget::pane {{ border: 1px solid {t.border}; border-radius: 6px; top: -1px; background: {t.surface}; }}
QTabBar::tab {{ background: transparent; color: {t.muted}; padding: 7px 14px; margin-right: 2px;
               border: 1px solid transparent; border-top-left-radius: 6px; border-top-right-radius: 6px; }}
QTabBar::tab:hover {{ color: {t.text}; background: {t.surface_alt}; }}
QTabBar::tab:selected {{ color: {t.text}; background: {t.surface}; border-color: {t.border};
                        border-bottom-color: {t.surface}; font-weight: 600; }}

QGroupBox {{ background: {t.surface}; border: 1px solid {t.border}; border-radius: 8px;
             margin-top: 14px; padding: 12px 10px 10px 10px; font-weight: 600; }}
QGroupBox::title {{ subcontrol-origin: margin; left: 12px; padding: 0 4px; color: {t.text}; }}

QLineEdit, QComboBox, QSpinBox, QPlainTextEdit {{
    background: {t.surface}; border: 1px solid {t.border}; border-radius: 5px; padding: 4px 6px;
    selection-background-color: {t.accent}; selection-color: {t.accent_text}; }}
QLineEdit:focus, QComboBox:focus, QSpinBox:focus, QPlainTextEdit:focus {{ border-color: {t.accent}; }}
QLineEdit:disabled, QComboBox:disabled, QSpinBox:disabled {{ color: {t.muted}; background: {t.surface_alt}; }}
QComboBox::drop-down {{ border: none; width: 20px; }}
QAbstractSpinBox::up-button, QAbstractSpinBox::down-button {{
    background: {t.surface_alt}; border: none; border-left: 1px solid {t.border}; width: 16px; }}
QAbstractSpinBox::up-button {{ border-top-right-radius: 5px; }}
QAbstractSpinBox::down-button {{ border-bottom-right-radius: 5px; }}
QAbstractSpinBox::up-button:hover, QAbstractSpinBox::down-button:hover {{ background: {t.accent_soft}; }}
QComboBox QAbstractItemView {{ background: {t.surface}; border: 1px solid {t.border};
                               selection-background-color: {t.accent_soft}; selection-color: {t.text}; }}

QPushButton {{ background: {t.surface}; border: 1px solid {t.border}; border-radius: 6px; padding: 5px 14px; }}
QPushButton:hover {{ border-color: {t.accent}; color: {t.accent}; }}
QPushButton:pressed {{ background: {t.accent_soft}; }}
QPushButton:default {{ background: {t.accent}; color: {t.accent_text}; border-color: {t.accent}; }}
QPushButton:disabled {{ color: {t.muted}; }}

QTableView, QTableWidget, QListWidget, QTreeView {{
    background: {t.surface}; alternate-background-color: {t.surface_alt}; border: 1px solid {t.border};
    border-radius: 6px; gridline-color: {t.border};
    selection-background-color: {t.accent_soft}; selection-color: {t.text}; }}
QTableView::item, QTableWidget::item {{ padding: 2px 6px; }}
QListWidget::item {{ padding: 5px 8px; border-radius: 4px; }}
QListWidget::item:selected {{ background: {t.accent_soft}; color: {t.text}; }}
QHeaderView::section {{ background: {t.surface_alt}; color: {t.muted}; border: none;
                        border-right: 1px solid {t.border}; border-bottom: 1px solid {t.border};
                        padding: 5px 6px; font-weight: 600; }}
QTableCornerButton::section {{ background: {t.surface_alt}; border: none; }}

QScrollBar:vertical {{ background: transparent; width: 11px; margin: 2px; }}
QScrollBar:horizontal {{ background: transparent; height: 11px; margin: 2px; }}
QScrollBar::handle {{ background: {t.border}; border-radius: 4px; min-height: 24px; min-width: 24px; }}
QScrollBar::handle:hover {{ background: {t.muted}; }}
QScrollBar::add-line, QScrollBar::sub-line {{ width: 0; height: 0; }}
QScrollBar::add-page, QScrollBar::sub-page {{ background: transparent; }}

QSplitter::handle {{ background: {t.window}; }}
QSplitter::handle:vertical {{ height: 6px; }}
QStatusBar {{ background: {t.surface}; border-top: 1px solid {t.border}; color: {t.muted}; }}
QStatusBar::item {{ border: none; }}
QProgressBar {{ border: 1px solid {t.border}; border-radius: 5px; background: {t.surface_alt};
               text-align: center; height: 14px; }}
QProgressBar::chunk {{ background: {t.accent}; border-radius: 4px; }}
QLabel#muted {{ color: {t.muted}; }}
QLabel#pageTitle {{ font-size: 15pt; font-weight: 600; }}
QLabel#danger {{ color: {t.danger}; }}
QLabel#ok {{ color: {t.ok}; }}
"""


def apply_theme(app: QApplication, mode: ThemeMode = ThemeMode.AUTO) -> Tokens:
    """Style the whole application; returns the colours in use."""
    global _current
    _current = resolve(mode, app)
    app.setStyle("Fusion")
    font = QFont()
    font.setFamilies(["Microsoft JhengHei UI", "Microsoft JhengHei", "Noto Sans CJK TC", "Segoe UI"])
    font.setPointSize(10)
    app.setFont(font)
    app.setPalette(palette(_current))
    app.setStyleSheet(style_sheet(_current))
    return _current


# ---------------------------------------------------------------- icons
# 24x24 stroke drawings; "{c}" is replaced with the colour.
_ICONS = {
    "open": '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
    "save": '<path d="M5 3h11l3 3v13a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/>'
            '<path d="M7 3v5h8V3"/><rect x="7" y="13" width="10" height="7" rx="1"/>',
    "undo": '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>',
    "redo": '<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>',
    "read": '<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>',
    "write": '<path d="M12 21V9"/><path d="m7 14 5-5 5 5"/><path d="M5 3h14"/>',
    "refresh": '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>',
    "users": '<circle cx="9" cy="8" r="3.5"/><path d="M3 20a6 6 0 0 1 12 0"/>'
             '<path d="M16 4.5a3.5 3.5 0 0 1 0 7"/><path d="M18 14.5a6 6 0 0 1 3 5.5"/>',
    "clean": '<path d="m7 21-4-4 11-11 6 6-8 9z"/><path d="M11 21h10"/><path d="m9 8 6 6"/>',
    "search": '<circle cx="11" cy="11" r="7"/><path d="m21 21-5-5"/>',
    "plus": '<path d="M12 5v14"/><path d="M5 12h14"/>',
    "chip": '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
    "up": '<path d="M12 19V5"/><path d="m6 11 6-6 6 6"/>',
    "down": '<path d="M12 5v14"/><path d="m18 13-6 6-6-6"/>',
    "trash": '<path d="M4 7h16"/><path d="M10 11v6"/><path d="M14 11v6"/>'
             '<path d="M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13"/><path d="M9 7V4h6v3"/>',
    "compare": '<path d="M8 4v16"/><path d="M16 4v16"/><path d="m4 8 4-4 4 4"/><path d="m12 16 4 4 4-4"/>',
    "warning": '<path d="M12 3 2 20h20z"/><path d="M12 10v4"/><path d="M12 17v.5"/>',
    "check": '<circle cx="12" cy="12" r="9"/><path d="m8 12 3 3 5-6"/>',
    "radio": '<rect x="6" y="7" width="12" height="15" rx="2"/><path d="M9 7V2"/>'
             '<rect x="8.5" y="10" width="7" height="4" rx="0.5"/><path d="M9 17h.5"/><path d="M12 17h.5"/>'
             '<path d="M15 17h.5"/>',
}


def icon(name: str, colour: str | None = None, size: int = 20) -> QIcon:
    """Render one of the stroke icons in ``colour`` (default: theme text colour)."""
    stroke = colour or _current.text
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="{stroke}" '
           f'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{_ICONS[name]}</svg>')
    renderer = QSvgRenderer(QByteArray(svg.encode()))
    result = QIcon()
    for scale in (1, 2):
        pix = QPixmap(QSize(size * scale, size * scale))
        pix.setDevicePixelRatio(scale)
        pix.fill(Qt.GlobalColor.transparent)
        painter = QPainter(pix)
        renderer.render(painter, QRectF(0, 0, size, size))
        painter.end()
        result.addPixmap(pix)
    return result


def app_icon() -> QIcon:
    """Window icon: the radio drawing on an accent-coloured rounded square."""
    svg = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
           f'<rect width="24" height="24" rx="5" fill="{LIGHT.accent}"/>'
           '<g fill="none" stroke="#ffffff" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" '
           'transform="translate(2 2) scale(0.83)">' + _ICONS["radio"] + "</g></svg>")
    renderer = QSvgRenderer(QByteArray(svg.encode()))
    result = QIcon()
    for size in (16, 32, 64, 256):
        pix = QPixmap(size, size)
        pix.fill(Qt.GlobalColor.transparent)
        painter = QPainter(pix)
        renderer.render(painter)
        painter.end()
        result.addPixmap(pix)
    return result
