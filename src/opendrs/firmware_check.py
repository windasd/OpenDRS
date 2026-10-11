"""Pre-flight checks before a firmware file is flashed (built-in IAP upgrade, iap.py, or
the vendor ``IAP+Flash.exe``). A wrong image can leave the radio unbootable, so the chosen
file is compared with what the radio reported on its last read:

* the firmware ID string (e.g. ``308E.B6.TARSCB.003``) is found inside the image;
* the same ID as the radio -> OK. The ID in the image is a template: at boot the
  firmware (FUN_0302074c) reads the RF chip ID and overwrites the two characters after
  ``<hw>.B`` ("BK" BK4819, "Ax" RDA1846/AT1846S, "6." 0x6818 = unchanged, "64" 0x7813),
  so a file ``308E.B6.TARSCB.003`` and a radio reporting ``308E.BBKTARSCB.003`` are the
  same build -> also OK;
* same hardware prefix, otherwise different -> warning, the user must confirm explicitly;
* a different hardware prefix or no ID -> blocked;
* the band-only patched images produced during the firmware analysis -> blocked (band limits
  widened; RF behaviour outside the vendor bands is unverified);
* the known record-read patched images (a vendor file + 30 bytes so the radio answers the
  PC call-recording download commands, fwpatch.py) -> warning that needs the explicit tick:
  verified on a radio; on a radio running the same build it is undone by flashing the
  vendor file again;
* the band-widened images with the record-read fix (and the VFO keypad fix), made on request
  for a test (mkpatch_band_record.py, mkpatch_keypad.py) -> the same kind of warning, saying
  what is unverified.
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

FIRMWARE_ID = re.compile(rb"(\d{3}[0-9A-F])\.([0-9A-Z][0-9A-Z.]{4,24}\.\d{3})\.")
BUILD_DATE = re.compile(rb"[A-Z][a-z]{2} [ 0-9]\d \d{4}")
MAX_IMAGE = 4 * 1024 * 1024
DEFAULT_IAP_EXE = Path(r"C:\Program Files (x86)\IAP(APP+FLASH)\IAP+Flash.exe")

# SHA-256 of the band-only patched images made during the analysis (never flash these).
BLOCKED_IMAGES = {
    "7d10b46b350ff818d7d9a6882baf9add21a093b6c1d4a683a759bbeb950830bf": "136-180／400-520 改頻段版",
    "7b623a85f0b29874b78e9f721cb8c564190b3dd409914012d87d90d0cf12c967": "136-180／400-520 改頻段版（改標示）",
}


# Vendor images whose hash is known (the file the user received from the vendor).
KNOWN_VENDOR_IMAGES = {
    "fd52d62d980c59039da869c05f2c5802fb60a9fc639f4cd6d73680c3b896cc36":
        "原廠 C7000_DR8500UV_4819_03（DR-8500UV／DH-98VU，136-174／400-480，Mar 9 2024）",
    "2267ee7a650490368bbf0de4197b542353864ef1ab82ea8cc54d9f20f0e526ab":
        "原廠 C7000_DR8500UV_4819_05（DR-8500UV／DH-98VU，144-146／430-440 業餘頻段，Mar 16 2024；與 03 版程式相同，只差頻段限制）",
}


RECORD_READ_NOTE = "這 30 bytes 的修補已於 2026-10-09 在 BK4819 實機（03 版）刷入並讀取 25 筆錄音驗證"
_BAND_WIDENED = ("頻段限制放寬為 136–180／400–520 MHz（對講機改回報 F136-520）：超出原廠 136–174／400–480 MHz "
                 "的部分沒有出廠校準，超出校準表最後節點時發射功率、頻偏與靜噪如何計算尚未查明，未經驗證，"
                 "發射測試請接假負載；錄音讀取的 30 bytes 與已實機驗證的版本相同。")
BAND_WIDENED_NOTE = (_BAND_WIDENED + "2026-10-11 已在 BK4819 實機（03 版）刷入，旋鈕可調到 5XX MHz，"
                     "但 VFO 鍵盤無法輸入 5 開頭的頻率（改用加了鍵盤修正的版本）")
BAND_WIDENED_KEYPAD_NOTE = (_BAND_WIDENED + "另修正 VFO 鍵盤首位數字只接受 1、4 的檢查，可輸入 5 開頭的頻率；"
                            "2026-10-11 已在 BK4819 實機（03 版）刷入，確認鍵盤可正常輸入 5XX MHz")

# Modified images made on request (mkpatch_record.py, mkpatch_band_record.py); flashable only
# after confirmation. SHA-256 -> (description, the vendor file it was made from, what is verified).
KNOWN_PATCHED_IMAGES = {
    "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252":
        ("錄音讀取修補版（原廠韌體改 30 bytes，讓對講機回應電腦讀取錄音的指令）",
         "C7000_DR8500UV_4819_03_136174_400480.bin", RECORD_READ_NOTE),
    "8f5a225f5dbe76706830ce99331e40605b55549695e30e7ab0f05b188ba3e7fa":
        ("錄音讀取修補版（業餘頻段 05 版原廠韌體改 30 bytes，讓對講機回應電腦讀取錄音的指令）",
         "C7000_DR8500UV_4819_05_144146_430440.bin", RECORD_READ_NOTE),
    "b31dc8893580c3c6e6ccc2ae922aefc9bd1a72bb1c756ae8339a9c037403cad3":
        ("136-180／400-520 改頻段測試版加錄音讀取修補（原廠韌體改 76 bytes）",
         "C7000_DR8500UV_4819_03_136174_400480.bin", BAND_WIDENED_NOTE),
    "8953e3e30285c0c55bff59dea8dd894f50a0816312f20758de9383f58e86d8ae":
        ("136-180／400-520 改頻段測試版加錄音讀取修補與鍵盤修正（原廠韌體改 108 bytes）",
         "C7000_DR8500UV_4819_03_136174_400480.bin", BAND_WIDENED_KEYPAD_NOTE),
}

# The two characters after "<hw>.B" in a firmware ID: the image holds the template "6.",
# which the firmware overwrites at boot according to the RF chip it finds (FUN_0302074c).
TEMPLATE_CHIP_CODE = "6."
RF_CHIP_CODES = {"BK": "BK4819", "Ax": "RDA1846／AT1846S", "6.": "0x6818", "64": "0x7813"}


class Level(str, Enum):
    OK = "ok"
    WARN = "warn"
    BLOCK = "block"


@dataclass(frozen=True)
class Finding:
    level: Level
    text: str


@dataclass(frozen=True)
class FirmwareFile:
    path: Path
    size: int
    sha256: str
    firmware_id: str  # "" when no ID string was found
    build_date: str

    @property
    def hardware(self) -> str:
        return self.firmware_id.split(".", 1)[0] if self.firmware_id else ""


def inspect(path: Path) -> FirmwareFile:
    data = Path(path).read_bytes()
    if len(data) > MAX_IMAGE:
        raise ValueError(f"檔案太大（{len(data):,} bytes），不像對講機韌體")
    match = FIRMWARE_ID.search(data)
    firmware_id = (match.group(1) + b"." + match.group(2)).decode("ascii") if match else ""
    date = BUILD_DATE.search(data, match.end() if match else 0)
    return FirmwareFile(Path(path), len(data), hashlib.sha256(data).hexdigest(), firmware_id,
                        " ".join(date.group(0).decode("ascii").split()) if date else "")


def _hardware(firmware_id: str) -> str:
    return firmware_id.split(".", 1)[0] if firmware_id else ""


def _chip_code(firmware_id: str) -> str | None:
    """The two characters after "<hw>.B" when they are a known RF-chip code, else None."""
    _hw, dot, rest = firmware_id.partition(".")
    code = rest[1:3]
    return code if dot and rest[:1] == "B" and code in RF_CHIP_CODES else None


def normalise_firmware_id(firmware_id: str) -> str:
    """``firmware_id`` with its RF-chip code put back to the template ("308E.BBK…" -> "308E.B6.…")."""
    if _chip_code(firmware_id) is None:
        return firmware_id
    hw, _dot, rest = firmware_id.partition(".")
    return f"{hw}.B{TEMPLATE_CHIP_CODE}{rest[3:]}"


def rf_chip(firmware_id: str) -> str | None:
    """The RF chip a radio-reported firmware ID indicates, None when the code is unknown."""
    code = _chip_code(firmware_id)
    return RF_CHIP_CODES[code] if code else None


def same_build(file_id: str, radio_id: str) -> bool:
    """True when the IDs are equal once the boot-time RF-chip code is ignored."""
    return bool(file_id) and normalise_firmware_id(file_id) == normalise_firmware_id(radio_id)


def _same_build_text(file_id: str, radio_id: str) -> str:
    file_code, radio_code = _chip_code(file_id), _chip_code(radio_id)
    template = "樣板" if file_code == TEMPLATE_CHIP_CODE else ""
    return (f"與對講機目前韌體為同一版本（對講機回報的 B{radio_code} 是開機時偵測到 {rf_chip(radio_id)} "
            f"射頻晶片後填入的代碼，檔案內為{template} B{file_code}）")


def _label(firmware_id: str, date: str) -> str:
    return f"{firmware_id}（{date}）" if date else firmware_id


def _patched_finding(name: str, vendor_file: str, note: str, image: FirmwareFile, radio_firmware: str,
                     radio_date: str) -> Finding:
    """A patched vendor image: undone by flashing the vendor file again, if that is the radio's build."""
    radio, base = _label(radio_firmware, radio_date), f"修補所用的原廠檔 {vendor_file} "
    no_way_back = "刷入後要改回對講機原本的韌體，須有原廠提供的該版檔案"
    dates_differ = bool(radio_date and image.build_date and radio_date != image.build_date)
    if radio_firmware and (dates_differ or not same_build(image.firmware_id, radio_firmware)):
        image_label = _label(image.firmware_id, image.build_date)
        way_back = f"對講機目前是 {radio}，{base}是 {image_label}，不是同一版本：{no_way_back}"
    elif radio_firmware and radio_date and image.build_date:
        way_back = f"對講機目前的 {radio}與{base}是同一版本，要復原時再刷一次這個原廠檔即可"
    else:  # no radio read yet, or a build date is missing
        current = f"對講機目前是 {radio}" if radio_firmware else "尚未從對講機讀取"
        way_back = f"{current}，無法確認與{base}是同一版本；若不是，{no_way_back}"
    return Finding(Level.WARN, f"這是{name}。{way_back}。{note}")


def check(image: FirmwareFile, radio_firmware: str, has_recent_backup: bool,
          radio_date: str = "") -> list[Finding]:
    """Everything the user should know before flashing ``image``; BLOCK means do not."""
    out: list[Finding] = []
    vendor = KNOWN_VENDOR_IMAGES.get(image.sha256)
    if vendor:
        out.append(Finding(Level.OK, f"已知的原廠韌體檔：{vendor}"))
    if image.sha256 in BLOCKED_IMAGES:
        out.append(Finding(Level.BLOCK, f"這是分析時產生的{BLOCKED_IMAGES[image.sha256]}：頻段限制被放寬，"
                                        "超出原廠頻段的射頻表現未經驗證，本程式不允許刷入"))
    patched = KNOWN_PATCHED_IMAGES.get(image.sha256)
    if patched:
        out.append(_patched_finding(*patched, image, radio_firmware, radio_date))
    if not image.firmware_id:
        out.append(Finding(Level.BLOCK, "檔案中找不到韌體 ID，無法確認是這款對講機的韌體"))
    elif not radio_firmware:
        out.append(Finding(Level.WARN, f"檔案韌體 {image.firmware_id}；尚未從對講機讀取過，無法比對目前版本"
                                       "（建議先「讀取對講機」）"))
    elif image.firmware_id == radio_firmware:
        out.append(Finding(Level.OK, f"韌體 ID 與對講機目前版本相同（{radio_firmware}）"))
    elif same_build(image.firmware_id, radio_firmware):
        out.append(Finding(Level.OK, _same_build_text(image.firmware_id, radio_firmware)))
    elif _hardware(image.firmware_id) == _hardware(radio_firmware):
        out.append(Finding(Level.WARN, f"同一硬體平台（{image.hardware}），但版本不同："
                                       f"檔案 {image.firmware_id}，對講機 {radio_firmware}，"
                                       "刷入前請確認來源可靠"))
    else:
        out.append(Finding(Level.BLOCK, f"硬體平台不同：檔案 {image.firmware_id}，對講機 {radio_firmware}"))
    if radio_date and image.build_date:
        if image.build_date == radio_date:
            out.append(Finding(Level.OK, f"編譯日期與對講機目前韌體相同（{radio_date}）"))
        else:
            out.append(Finding(Level.WARN, f"編譯日期不同：檔案 {image.build_date}，對講機 {radio_date}"))
    out.append(Finding(Level.OK, "已有今天從對講機讀取的備份") if has_recent_backup
               else Finding(Level.WARN, "今天還沒有從對講機讀取的備份；刷韌體前建議先「讀取對講機」保留設定"))
    return out


def verdict(findings: list[Finding]) -> Level:
    levels = {f.level for f in findings}
    return Level.BLOCK if Level.BLOCK in levels else Level.WARN if Level.WARN in levels else Level.OK
