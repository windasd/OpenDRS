# 第三方元件授權 / Third-party notices

`OpenDRS.exe` 由 PyInstaller 打包，隨附下列元件（位於 `_internal\`）。授權全文在同一個 `licenses\` 資料夾。

| 元件 | 授權 | 全文 | 原始碼 |
|---|---|---|---|
| Qt 6（`Qt6*.dll`、`plugins\`） | LGPL-3.0 | `LGPL-3.0.txt`、`GPL-3.0.txt` | https://code.qt.io/ ・ https://download.qt.io/official_releases/qt/ |
| PySide6、shiboken6（Qt for Python） | LGPL-3.0 | `LGPL-3.0.txt`、`GPL-3.0.txt` | https://code.qt.io/cgit/pyside/pyside-setup.git/ |
| Python 3（`python3*.dll`、標準函式庫，及其附帶的 OpenSSL、libffi、bzip2、xz、zlib、mpdecimal 等） | PSF License 及各元件授權 | `Python-LICENSE.txt` | https://www.python.org/downloads/source/ |
| pySerial | BSD-3-Clause | `pyserial-LICENSE.txt` | https://github.com/pyserial/pyserial |
| PyInstaller 啟動程式（bootloader） | GPL-2.0-or-later，附允許散布打包程式的例外條款 | `PyInstaller-COPYING.txt` | https://github.com/pyinstaller/pyinstaller |

Qt 與 PySide6 以 LGPL-3.0 使用：它們是 `_internal\` 內獨立、可替換的動態連結程式庫；本程式未修改其原始碼。
使用者可以用相容版本的同名檔案替換它們。

本程式不隨附、也不重製原廠的 `IAP+Flash.exe`、`Wav.exe`、`ambep2.exe`（DVSI）或任何原廠韌體；
需要時由使用者自行指定其位置。

The Qt and PySide6 libraries are used under the LGPL-3.0 as separate, replaceable DLLs and are
unmodified. Vendor tools (IAP+Flash.exe, Wav.exe, DVSI ambep2.exe) and vendor firmware are not
included.
