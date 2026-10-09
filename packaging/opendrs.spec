# PyInstaller spec for the Windows build. Run `python packaging/build.py`, which creates
# build/opendrs.ico and build/version_info.txt first, then self-tests and zips the result.
# -*- mode: python -*-
from fnmatch import fnmatch
from pathlib import Path

ROOT = Path(SPECPATH).parent  # noqa: F821 - provided by PyInstaller
BUILD = ROOT / "build"
ICON = BUILD / "opendrs.ico"
VERSION = BUILD / "version_info.txt"

# Qt parts a widgets application never loads (~55 MB). They arrive through plugins: the
# on-screen keyboard plugin pulls in Qt Quick/QML, the PDF *image-format* plugin pulls in
# Qt Pdf -> Qt Network (PDF export itself is QPdfWriter in Qt6Gui). Checked with
# PyInstaller's bindepend: nothing kept imports these, and `--self-test` exercises the rest.
UNUSED = (
    "*/opengl32sw.dll",  # software OpenGL fallback
    "*/qtvirtualkeyboardplugin.dll", "*/qt6virtualkeyboard*.dll", "*/qt6quick*.dll", "*/qt6qml*.dll",
    "*/qpdf.dll", "*/qt6pdf.dll",
    "*/qtuiotouchplugin.dll",  # TUIO touch input over the network
    "*/qt6network.dll", "*/qtnetwork.pyd", "*/plugins/tls/*", "*/plugins/networkinformation/*",
    "*/qt6opengl.dll",
    "libcrypto-3-x64.dll", "libssl-3-x64.dll",  # Qt's OpenSSL; Python's _ssl uses libcrypto-3.dll
)
KEEP_TRANSLATIONS = "*_zh_tw.qm"  # gui/translations.py loads qtbase_zh_TW


def _wanted(entry) -> bool:
    dest = entry[0].replace("\\", "/").lower()
    if "/translations/" in dest and dest.endswith(".qm"):
        return fnmatch(dest, "*/" + KEEP_TRANSLATIONS)
    return not any(fnmatch(dest, pattern) for pattern in UNUSED)


a = Analysis(  # noqa: F821
    [str(ROOT / "packaging" / "launch.py")],
    pathex=[str(ROOT / "src")],
    # pywin32 is reachable only through logging.handlers.NTEventLogHandler, which the app never uses
    excludes=["tkinter", "pytest", "_pytest", "win32api", "win32evtlog", "win32evtlogutil", "win32con",
              "pywintypes", "pythoncom"],
)
a.binaries = [b for b in a.binaries if _wanted(b)]
a.datas = [d for d in a.datas if _wanted(d)]
pyz = PYZ(a.pure)  # noqa: F821
exe = EXE(  # noqa: F821
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="OpenDRS",
    console=False,
    upx=False,  # UPX-packed executables trip antivirus heuristics
    icon=str(ICON) if ICON.is_file() else None,
    version=str(VERSION) if VERSION.is_file() else None,
)
coll = COLLECT(exe, a.binaries, a.datas, name="OpenDRS", upx=False)  # noqa: F821
