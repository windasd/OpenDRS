from __future__ import annotations

import os
from pathlib import Path

import pytest

from opendrs.firmware_check import (
    BLOCKED_IMAGES,
    KNOWN_PATCHED_IMAGES,
    KNOWN_VENDOR_IMAGES,
    Finding,
    FirmwareFile,
    Level,
    check,
    inspect,
    normalise_firmware_id,
    rf_chip,
    same_build,
    verdict,
)

RADIO = "308E.BBKTARSCB.003"
VENDOR = Path(__file__).resolve().parents[2] / "C7000_DR8500UV_4819_03_136174_400480.bin"
RECORD_PATCH = Path(__file__).resolve().parents[2] / "DR8500UV_B6_record_read.bin"
RECORD_PATCH_SHA256 = "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252"


def _image(tmp_path: Path, firmware_id: bytes, date: bytes = b"Mar  9 2024", name: str = "fw.bin") -> Path:
    path = tmp_path / name
    path.write_bytes(b"\x00" * 64 + firmware_id + b"\x00" * 32 + date + b"\x0017:02:38" + b"\xff" * 64)
    return path


def test_inspect_reads_id_and_date(tmp_path: Path) -> None:
    img = inspect(_image(tmp_path, b"308E.B6.TARSCB.003."))
    assert (img.firmware_id, img.build_date, img.hardware) == ("308E.B6.TARSCB.003", "Mar 9 2024", "308E")
    assert len(img.sha256) == 64 and img.size > 0
    blank = inspect(_image(tmp_path, b"no id here", b"", "blank.bin"))
    assert blank.firmware_id == "" and blank.build_date == ""


@pytest.mark.parametrize(("radio_id", "chip"), [
    ("308E.BBKTARSCB.003", "BK4819"),
    ("308E.BAxTARSCB.003", "RDA1846／AT1846S"),
    ("308E.B6.TARSCB.003", "0x6818"),
    ("308E.B64TARSCB.003", "0x7813"),
])
def test_rf_chip_codes_normalise_to_the_template(radio_id: str, chip: str) -> None:
    assert normalise_firmware_id(radio_id) == "308E.B6.TARSCB.003"
    assert rf_chip(radio_id) == chip
    assert same_build("308E.B6.TARSCB.003", radio_id) and same_build(radio_id, "308E.B6.TARSCB.003")


def test_unknown_chip_code_is_a_real_difference() -> None:
    for unknown in ("308E.BXYTARSCB.003", "308E.B6xTARSCB.003", "308E.CBKTARSCB.003", "308EBBKTARSCB", ""):
        assert normalise_firmware_id(unknown) == unknown and rf_chip(unknown) is None
    assert not same_build("308E.B6.TARSCB.003", "308E.BXYTARSCB.003")
    assert not same_build("308E.B6.TARSCB.003", "308E.BBKTARSCB.004")  # another version
    assert not same_build("", "") and not same_build("308E.B6.TARSCB.003", "")
    assert normalise_firmware_id("1234.BAxQ") == "1234.B6.Q"  # the slot follows the hardware prefix


def test_verdicts(tmp_path: Path) -> None:
    same = inspect(_image(tmp_path, b"308E.BBKTARSCB.003."))
    findings = check(same, RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.OK and any("相同" in f.text for f in findings)
    template = inspect(_image(tmp_path, b"308E.B6.TARSCB.003.", name="b6.bin"))
    findings = check(template, RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.OK  # the same build: the radio filled in BK for its BK4819
    assert findings[0].text == ("與對講機目前韌體為同一版本（對講機回報的 BBK 是開機時偵測到 BK4819 射頻晶片後"
                                "填入的代碼，檔案內為樣板 B6.）")
    newer = inspect(_image(tmp_path, b"308E.B6.TARSCB.004.", name="b6-004.bin"))
    assert verdict(check(newer, RADIO, True, "Mar 9 2024")) is Level.WARN
    unknown_chip = inspect(_image(tmp_path, b"308E.BXYTARSCB.003.", name="xy.bin"))
    findings = check(unknown_chip, RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.WARN and "版本不同" in findings[0].text
    other = inspect(_image(tmp_path, b"412A.XX.TARSCB.001.", name="other.bin"))
    assert verdict(check(other, RADIO, True)) is Level.BLOCK
    unknown_radio = check(template, "", True)
    assert verdict(unknown_radio) is Level.WARN and "尚未從對講機讀取" in unknown_radio[0].text
    no_backup = check(same, RADIO, False, "Mar 9 2024")
    assert verdict(no_backup) is Level.WARN and "備份" in no_backup[-1].text
    older = check(same, RADIO, True, "Dec 19 2023")
    assert verdict(older) is Level.WARN and "編譯日期不同" in older[-2].text
    blank = inspect(_image(tmp_path, b"nothing", name="blank.bin"))
    assert verdict(check(blank, RADIO, True)) is Level.BLOCK


def test_too_large_file_is_rejected(tmp_path: Path) -> None:
    big = tmp_path / "big.bin"
    big.write_bytes(bytes(4 * 1024 * 1024 + 1))
    with pytest.raises(ValueError, match="太大"):
        inspect(big)


@pytest.mark.skipif(not VENDOR.is_file(), reason="vendor firmware not on this machine")
def test_vendor_file_and_band_patches() -> None:
    img = inspect(VENDOR)
    assert img.sha256 in KNOWN_VENDOR_IMAGES and img.firmware_id == "308E.B6.TARSCB.003"
    findings = check(img, RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.OK  # the radio's BBK is this build with the BK4819 code filled in
    assert any("原廠" in f.text for f in findings) and any("編譯日期與對講機目前韌體相同" in f.text for f in findings)
    assert any("同一版本" in f.text and "BK4819" in f.text for f in findings)
    assert verdict(check(img, RADIO, False, "Mar 9 2024")) is Level.WARN  # no backup today
    for patched in VENDOR.parent.glob("DR8500UV_136-180_400-520*.bin"):
        p = inspect(patched)
        band = check(p, RADIO, True, "Mar 9 2024")
        assert p.sha256 in BLOCKED_IMAGES and verdict(band) is Level.BLOCK
        assert any(f.level is Level.BLOCK and "頻段限制被放寬" in f.text for f in band)


def test_record_read_patch_is_an_untested_warning() -> None:
    def image(sha256: str) -> FirmwareFile:
        return FirmwareFile(Path("fw.bin"), 1024, sha256, "308E.B6.TARSCB.003", "Mar 9 2024")

    def patch_findings(radio: str, date: str = "Mar 9 2024") -> list[Finding]:
        return [f for f in check(image(RECORD_PATCH_SHA256), radio, True, date) if "錄音讀取修補版" in f.text]

    assert RECORD_PATCH_SHA256 in KNOWN_PATCHED_IMAGES
    vendor_file = KNOWN_PATCHED_IMAGES[RECORD_PATCH_SHA256][1]
    findings = check(image(RECORD_PATCH_SHA256), RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.WARN  # needs the risk tick, but is not blocked
    patch = patch_findings(RADIO)
    assert len(patch) == 1 and patch[0].level is Level.WARN
    assert "30 bytes" in patch[0].text and RADIO in patch[0].text and "實機" in patch[0].text
    assert vendor_file in patch[0].text and "再刷一次" in patch[0].text and "原廠提供" not in patch[0].text
    assert any(f.level is Level.OK and "同一版本" in f.text for f in findings)  # BBK = B6 + BK4819 code
    cases = (("", "", "無法確認"), (RADIO, "", "無法確認"),
             ("308E.B6.TARSCB.004", "Mar 9 2024", "不是同一版本"), (RADIO, "Dec 19 2023", "不是同一版本"))
    for radio, date, wording in cases:
        cautious = patch_findings(radio, date)
        assert len(cautious) == 1 and cautious[0].level is Level.WARN and "實機" in cautious[0].text
        assert wording in cautious[0].text and "原廠提供" in cautious[0].text
        assert "再刷一次" not in cautious[0].text
    band = check(image(next(iter(BLOCKED_IMAGES))), RADIO, True, "Mar 9 2024")
    assert verdict(band) is Level.BLOCK and not any("錄音讀取修補版" in f.text for f in band)
    plain = check(image("0" * 64), RADIO, True, "Mar 9 2024")
    assert not any("錄音讀取修補版" in f.text for f in plain)


@pytest.mark.skipif(not RECORD_PATCH.is_file(), reason="record-read patched firmware not on this machine")
def test_record_read_patch_file() -> None:
    img = inspect(RECORD_PATCH)
    assert img.sha256 in KNOWN_PATCHED_IMAGES and img.firmware_id == "308E.B6.TARSCB.003"
    findings = check(img, RADIO, True, "Mar 9 2024")
    assert verdict(findings) is Level.WARN  # not BLOCK: the dialog's explicit confirmation applies
    assert any(f.level is Level.WARN and "錄音讀取修補版" in f.text and RADIO in f.text for f in findings)
    vendor_file = KNOWN_PATCHED_IMAGES[img.sha256][1]
    assert vendor_file == VENDOR.name  # the way back is the vendor file in the same folder
    assert any(vendor_file in f.text and "實機" in f.text for f in findings)


def test_firmware_dialog_gates_the_launch(tmp_path: Path) -> None:
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtGui import QGuiApplication
    from PySide6.QtWidgets import QApplication

    from opendrs.gui.firmware_dialog import FirmwareDialog

    QApplication.instance() or QApplication([])
    exe = tmp_path / "IAP+Flash.exe"
    exe.write_bytes(b"MZ")
    launched: list[tuple[Path, Path]] = []
    dialog = FirmwareDialog(RADIO, "Mar 9 2024", True, exe, False,
                            launcher=lambda e, d: launched.append((e, d)) or True)
    assert not dialog.launch.isEnabled()  # no file yet
    dialog.set_file(_image(tmp_path, b"308E.B6.TARSCB.004.", name="b6-004.bin"))
    assert not dialog.launch.isEnabled() and dialog.confirm.isEnabled()  # another version: needs the tick
    dialog.confirm.setChecked(True)
    assert dialog.launch.isEnabled() and dialog.start_tool()
    assert launched == [(exe, tmp_path)] and QGuiApplication.clipboard().text().endswith("b6-004.bin")
    dialog.set_file(_image(tmp_path, b"412A.XX.TARSCB.001.", name="other.bin"))
    assert not dialog.launch.isEnabled() and not dialog.confirm.isEnabled()  # blocked
    dialog.set_file(_image(tmp_path, b"308E.B6.TARSCB.003.", name="b6.bin"))
    assert dialog.launch.isEnabled() and not dialog.confirm.isChecked()  # same build as the BBK radio
    dialog.set_file(_image(tmp_path, b"308E.BBKTARSCB.003.", name="same.bin"))
    assert dialog.launch.isEnabled()
    dialog.exe.setText(str(tmp_path / "missing.exe"))
    assert not dialog.launch.isEnabled()
    busy = FirmwareDialog(RADIO, "", True, exe, True, launcher=lambda e, d: True)
    busy.set_file(_image(tmp_path, b"308E.BBKTARSCB.003.", name="same2.bin"))
    assert not busy.launch.isEnabled()
    assert not dialog.flash.isEnabled()  # no port selected: built-in upgrade unavailable


def _builtin_dialog(tmp_path: Path, monkeypatch, transport, answer: str = "Yes"):  # noqa: ANN001, ANN202
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtWidgets import QApplication, QMessageBox

    from opendrs.gui.firmware_dialog import FirmwareDialog

    app = QApplication.instance() or QApplication([])
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: getattr(QMessageBox.StandardButton, answer))
    dialog = FirmwareDialog(RADIO, "Mar 9 2024", True, tmp_path / "none.exe", False,
                            port_label="COM3", transport=transport)
    path = _image(tmp_path, b"308E.BBKTARSCB.003.", name="same.bin")
    dialog.set_file(path)
    dialog.confirm.setChecked(True)
    dialog.iap_mode.setChecked(True)
    return app, dialog, path


def test_builtin_upgrade_runs_against_simulated_bootloader(tmp_path: Path, monkeypatch) -> None:
    from opendrs.iap import PAGE_SIZE
    from opendrs.iap_sim import SimulatedBootloader

    boot = SimulatedBootloader()
    app, dialog, path = _builtin_dialog(tmp_path, monkeypatch, lambda: boot)
    dialog.set_file(path)  # choosing a file clears the risk tick
    # same build, backup present, built-in upgrade verified on hardware: nothing to confirm
    assert not dialog.confirm.isChecked() and not dialog.confirm.isEnabled() and dialog.flash.isEnabled()
    assert any("已在實機驗證" in dialog.findings.item(i).text() for i in range(dialog.findings.count()))
    dialog.iap_mode.setChecked(False)
    assert not dialog.flash.isEnabled()  # the radio must be in IAP mode
    dialog.iap_mode.setChecked(True)
    assert dialog.flash.isEnabled() and not dialog.launch.isEnabled()  # vendor exe missing
    assert dialog.start_flash()
    dialog.wait()
    app.processEvents()
    data = path.read_bytes()
    assert dialog.upgrade_result is not None and dialog.error == "" and boot.finished
    assert bytes(boot.flash[:len(data)]) == data and len(boot.flash) == PAGE_SIZE
    assert "升級完成" in dialog.status.text() and "IAP Erase ok" in dialog.status.text()
    assert not dialog.iap_mode.isChecked() and not dialog.flash.isEnabled()  # re-confirm before another run


def test_builtin_upgrade_reports_a_silent_radio(tmp_path: Path, monkeypatch) -> None:
    from opendrs.iap_sim import SimulatedBootloader

    app, dialog, _ = _builtin_dialog(tmp_path, monkeypatch, lambda: SimulatedBootloader(silent=True))
    assert dialog.start_flash()
    dialog.wait()
    app.processEvents()
    assert dialog.upgrade_result is None and "升級未完成，清除指令已送出" in dialog.status.text()


def test_builtin_upgrade_port_failure_and_declined_confirmation(tmp_path: Path, monkeypatch) -> None:
    from opendrs.radio_link import RadioError

    def busy_port():  # noqa: ANN202
        raise RadioError("無法開啟 COM3：拒絕存取")

    app, dialog, _ = _builtin_dialog(tmp_path, monkeypatch, busy_port)
    assert dialog.start_flash()
    dialog.wait()
    app.processEvents()
    assert dialog.status.text() == "升級未開始，無法開啟 COM3：拒絕存取"
    _app, declined, _ = _builtin_dialog(tmp_path, monkeypatch, busy_port, answer="No")
    assert not declined.start_flash() and declined._worker is None


def test_builtin_upgrade_rechecks_a_file_changed_after_the_check(tmp_path: Path, monkeypatch) -> None:
    from opendrs.iap_sim import SimulatedBootloader

    boot = SimulatedBootloader()
    _app, dialog, path = _builtin_dialog(tmp_path, monkeypatch, lambda: boot)
    path.write_bytes(path.read_bytes() + b"tampered")
    assert not dialog.start_flash() and not boot.sent
    assert "被修改過" in dialog.status.text() and not dialog.confirm.isChecked()


def test_builtin_upgrade_cannot_be_closed_while_running_but_can_be_stopped(tmp_path: Path, monkeypatch) -> None:
    import time

    class QuietPort:  # opens fine, never answers
        def __init__(self) -> None:
            self.sent: list[bytes] = []

        def write(self, data: bytes) -> None:
            self.sent.append(data)

        def read_idle(self, idle: float, max_wait: float) -> bytes:
            time.sleep(0.02)
            return b""

        def reset_input(self) -> None:
            pass

        def close(self) -> None:
            pass

    port = QuietPort()
    app, dialog, _ = _builtin_dialog(tmp_path, monkeypatch, lambda: port)
    dialog.show()
    assert dialog.start_flash()
    assert not dialog.flash.isEnabled() and not dialog.launch.isEnabled() and dialog.stop.isVisible()
    dialog.reject()
    assert dialog.isVisible() and "升級進行中" in dialog.status.text()
    deadline = time.monotonic() + 5
    while not port.sent and time.monotonic() < deadline:  # stop while waiting for the erase reply
        time.sleep(0.01)
    dialog.stop.click()
    assert not dialog.stop.isEnabled()
    dialog.wait(5000)
    app.processEvents()
    assert "升級未完成，已停止" in dialog.status.text() and not dialog.stop.isVisible()
    dialog.reject()
    assert not dialog.isVisible()
