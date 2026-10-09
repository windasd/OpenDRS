"""Record-read firmware patch (fwpatch.py) and its button in the firmware dialog."""

from __future__ import annotations

import hashlib
import os
import struct
from pathlib import Path

import pytest

from opendrs import fwpatch
from opendrs.firmware_check import KNOWN_PATCHED_IMAGES, KNOWN_VENDOR_IMAGES

HERE = Path(__file__).resolve()
VENDOR_03 = HERE.parents[2] / "C7000_DR8500UV_4819_03_136174_400480.bin"
VENDOR_05 = Path.home() / "Downloads" / "hora dh-98vu" / "韌體" / "C7000_DR8500UV_4819_05_144146_430440.bin"
PATCHED_03 = "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252"  # flashed and verified on a radio
PATCHED_05 = "8f5a225f5dbe76706830ce99331e40605b55549695e30e7ab0f05b188ba3e7fa"
SIZE = 841_904


def synthetic(extra: dict[int, bytes] | None = None) -> bytes:
    """An image that only has the original bytes at the patch sites (everything else zero)."""
    img = bytearray(SIZE)
    for site in fwpatch.RECORD_READ:
        o = site.va - fwpatch.A_BASE
        img[o:o + len(site.original)] = site.original
    for va, data in (extra or {}).items():
        o = va - fwpatch.A_BASE
        img[o:o + len(data)] = data
    return bytes(img)


def test_patch_sites_are_known_hashes() -> None:
    assert set(fwpatch.KNOWN_BASES) <= set(KNOWN_VENDOR_IMAGES)
    assert {PATCHED_03, PATCHED_05} <= set(KNOWN_PATCHED_IMAGES)
    assert sum(len(s.patched) for s in fwpatch.RECORD_READ) == 30


def test_synthetic_image_is_patched_after_the_safety_scans() -> None:
    img = synthetic()
    assert fwpatch.record_read_status(img) == "patchable"
    result = fwpatch.patch_record_read(img)
    assert result.changed == 30 and result.base_name == "" and fwpatch.is_record_read_patched(result.data)
    assert fwpatch.record_read_status(result.data) == "patched"
    with pytest.raises(fwpatch.PatchError, match="已經是錄音讀取版"):
        fwpatch.patch_record_read(result.data)


def test_refuses_other_code_and_live_code_in_the_cave() -> None:
    wrong = bytearray(synthetic())
    wrong[0x03026E5C - fwpatch.A_BASE] ^= 1
    assert "修補點不符" in fwpatch.record_read_status(bytes(wrong))
    with pytest.raises(fwpatch.PatchError, match="修補點不符"):
        fwpatch.patch_record_read(bytes(wrong))
    # a bsr32 somewhere else that calls into the dead code: the cave would not be dead
    at, target = 0x03030000, 0x03027030
    off = ((target - at) // 2) & 0x3FFFFFF
    call = struct.pack("<HH", (0xE000_0000 | off) >> 16, off & 0xFFFF)
    with pytest.raises(fwpatch.PatchError, match="進入被覆寫的區域"):
        fwpatch.patch_record_read(synthetic({at: call}))
    assert "太小" in fwpatch.record_read_status(b"\0" * 100)


@pytest.mark.skipif(not VENDOR_03.is_file(), reason="vendor firmware not on this machine")
def test_vendor_03_gives_the_image_verified_on_the_radio() -> None:
    result = fwpatch.patch_record_read(VENDOR_03.read_bytes())
    assert result.sha256 == PATCHED_03 and result.base_name == VENDOR_03.name


@pytest.mark.skipif(not VENDOR_05.is_file(), reason="ham-band vendor firmware not on this machine")
def test_vendor_05_ham_band_build() -> None:
    data = VENDOR_05.read_bytes()
    assert hashlib.sha256(data).hexdigest() in fwpatch.KNOWN_BASES
    result = fwpatch.patch_record_read(data)
    assert result.sha256 == PATCHED_05 and result.changed == 30


def test_dialog_makes_and_selects_the_record_read_version(tmp_path: Path) -> None:
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtWidgets import QApplication

    from opendrs.gui.firmware_dialog import FirmwareDialog

    QApplication.instance() or QApplication([])
    src = tmp_path / "vendor.bin"
    src.write_bytes(synthetic())
    dialog = FirmwareDialog("308E.BBKTARSCB.003", "Mar 9 2024", True, tmp_path / "none.exe", False)
    assert not dialog.make_patch.isEnabled()  # no file yet
    dialog.set_file(src)
    assert dialog.make_patch.isEnabled() and "產生錄音讀取版" in dialog.file_info.text()
    assert dialog.make_record_read(src) is None and "不要覆蓋原廠韌體" in dialog.status.text()
    out = dialog.make_record_read(tmp_path / "vendor_record_read.bin")
    assert out is not None and fwpatch.is_record_read_patched(out.read_bytes())
    assert dialog.file.text() == str(out) and "錄音讀取版" in dialog.file_info.text()
    assert not dialog.make_patch.isEnabled() and "改 30 bytes" in dialog.status.text()
    assert src.read_bytes() == synthetic()  # the vendor file is left untouched
