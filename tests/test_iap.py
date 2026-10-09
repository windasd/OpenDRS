"""IAP firmware upgrade: paging, command bytes, and a full run against a simulated bootloader."""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from opendrs.iap import PAGE_SIZE, IapClient, IapError, IapImage
from opendrs.iap_sim import SimulatedBootloader

VENDOR = Path(__file__).resolve().parents[2] / "C7000_DR8500UV_4819_03_136174_400480.bin"
VENDOR_TX_SHA256 = "04e16d9c9daa9591f00e8bf833701f8d73cebcfefbd753d9029f2d3fff064e2e"
FIXTURES = Path(__file__).parent / "fixtures" / "iap"


def _image(size: int) -> bytes:
    return bytes((i * 7 + 3) & 0xFF for i in range(size))


def test_paging_matches_the_vendor_tool() -> None:
    img = IapImage.from_bytes(_image(PAGE_SIZE * 2 + 5))
    assert img.last_page == 2 and len(img.pages) == 3
    assert img.pages[2] == _image(PAGE_SIZE * 2 + 5)[PAGE_SIZE * 2:] + bytes(PAGE_SIZE - 5)
    # exact multiple: the vendor tool still sends len // 2048 + 1 pages
    exact = IapImage.from_bytes(_image(PAGE_SIZE * 2))
    assert exact.last_page == 2 and exact.pages[2] == bytes(PAGE_SIZE)
    assert img.erase_command() == b"Erase      " + bytes(4) + (2).to_bytes(4, "big")
    assert len(img.stream()) == 19 + 3 * PAGE_SIZE


def test_rejects_bad_input() -> None:
    with pytest.raises(ValueError, match="空"):
        IapImage.from_bytes(b"")
    with pytest.raises(ValueError, match="太大"):
        IapImage.from_bytes(bytes(4 * 1024 * 1024 + 1))
    with pytest.raises(ValueError, match="位址"):
        IapImage.from_bytes(b"x", address=-1)


def test_full_upgrade_against_simulated_bootloader() -> None:
    data = _image(PAGE_SIZE * 5 + 1234)
    img = IapImage.from_bytes(data, address=0x00004000)
    boot = SimulatedBootloader()
    steps: list[tuple[int, int, str]] = []
    result = IapClient(boot, lambda d, t, s: steps.append((d, t, s))).upgrade(img)
    assert boot.finished and boot.address == 0x4000 and boot.last_page == 5
    assert bytes(boot.flash[:len(data)]) == data and len(boot.flash) == 6 * PAGE_SIZE
    assert bytes(boot.sent) == img.stream()
    assert result.checksum == img.checksum == sum(boot.flash) and result.pages == 6
    assert "ok" in result.erase_reply and steps[-1] == (6, 6, "升級完成")


class ScriptedTransport:
    """Hands out ``chunks`` one per read (``b""`` = an empty read, like a quiet serial port)."""

    def __init__(self, chunks: list[bytes], fail_on_write: int | None = None, fail_reset: bool = False) -> None:
        self.chunks = list(chunks)
        self.sent: list[bytes] = []
        self.fail_on_write = fail_on_write
        self.fail_reset = fail_reset

    def write(self, data: bytes) -> None:
        if self.fail_on_write is not None and len(self.sent) == self.fail_on_write:
            raise OSError("WriteFile failed")
        self.sent.append(data)

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        return self.chunks.pop(0) if self.chunks else b""

    def read_exact(self, n: int, timeout: float) -> bytes:
        raise AssertionError("not used")

    def reset_input(self) -> None:
        if self.fail_reset:
            raise OSError("ClearCommError failed")

    def close(self) -> None:
        pass


def _checksum_reply(img: IapImage) -> list[bytes]:
    tail = b"ChecksumI" + img.checksum.to_bytes(4, "big")
    return [tail[:5], b"", tail[5:11], tail[11:]]


def test_replies_split_across_reads_and_empty_reads() -> None:
    img = IapImage.from_bytes(_image(PAGE_SIZE + 3))
    t = ScriptedTransport([b"", b"IAP Erase o", b"", b"k\r\n", b"k", b"yd"] + _checksum_reply(img))
    result = IapClient(t).upgrade(img)
    assert result.checksum == img.checksum and b"".join(t.sent) == img.stream()


def test_last_page_may_answer_with_the_checksum_only() -> None:
    img = IapImage.from_bytes(_image(PAGE_SIZE + 3))
    t = ScriptedTransport([b"ok", b"kyd", b"ChecksumI" + img.checksum.to_bytes(4, "big")])
    assert IapClient(t).upgrade(img).pages == 2


def test_silence_after_erase_times_out_and_says_it_was_sent() -> None:
    client = IapClient(ScriptedTransport([]))
    client.ERASE_TIMEOUT = 0.05
    with pytest.raises(IapError, match="清除指令已送出，但沒有收到") as info:
        client.upgrade(IapImage.from_bytes(_image(100)))
    assert info.value.erase_sent is True


def test_no_reply_to_erase() -> None:
    with pytest.raises(IapError, match="升級（IAP）模式") as info:
        IapClient(SimulatedBootloader(silent=True)).upgrade(IapImage.from_bytes(_image(100)))
    assert info.value.erase_sent is True  # sent, even though nothing came back


@pytest.mark.parametrize("boot, text", [
    (SimulatedBootloader(stop_acking_after=2), "第 3/4 頁沒有收到確認"),
    (SimulatedBootloader(bad_ack=b"err"), "第 1/4 頁沒有收到確認"),
    (SimulatedBootloader(corrupt_checksum=True), "校驗和不符.*請勿直接開機"),
])
def test_failures_after_erase_say_so(boot: SimulatedBootloader, text: str) -> None:
    with pytest.raises(IapError, match=text) as info:
        IapClient(boot).upgrade(IapImage.from_bytes(_image(PAGE_SIZE * 3 + 1)))
    assert info.value.erase_sent is True and "升級模式" in str(info.value)


def test_cancel_before_erase_sends_nothing() -> None:
    boot = SimulatedBootloader()
    with pytest.raises(IapError, match="未送出任何資料") as info:
        IapClient(boot, cancelled=lambda: True).upgrade(IapImage.from_bytes(_image(100)))
    assert info.value.erase_sent is False and not boot.sent


def test_cancel_while_waiting_for_erase() -> None:
    t = ScriptedTransport([])
    calls = iter([False, False, False, True])  # before erase, then while waiting
    with pytest.raises(IapError, match="已停止") as info:
        IapClient(t, cancelled=lambda: next(calls)).upgrade(IapImage.from_bytes(_image(100)))
    assert info.value.erase_sent is True and len(t.sent) == 1


def test_cancel_between_pages() -> None:
    boot = SimulatedBootloader()
    client = IapClient(boot, cancelled=lambda: len(boot.flash) >= 2 * PAGE_SIZE)
    with pytest.raises(IapError, match="已停止") as info:
        client.upgrade(IapImage.from_bytes(_image(PAGE_SIZE * 3 + 1)))
    assert info.value.erase_sent and len(boot.flash) == 2 * PAGE_SIZE


@pytest.mark.parametrize("fail_on_write, fail_reset, erase_sent", [
    (None, True, False),  # failed before anything was written
    (0, False, True),     # the erase command itself may be partly out: assume it was sent
    (2, False, True),
])
def test_serial_error_reports_whether_erase_was_sent(fail_on_write: int | None, fail_reset: bool,
                                                     erase_sent: bool) -> None:
    t = ScriptedTransport([b"ok", b"kyd", b"kyd"], fail_on_write=fail_on_write, fail_reset=fail_reset)
    with pytest.raises(IapError, match="連線中斷") as info:
        IapClient(t).upgrade(IapImage.from_bytes(_image(PAGE_SIZE * 3)))
    assert info.value.erase_sent is erase_sent


def test_tolerates_extra_text_in_replies() -> None:
    boot = SimulatedBootloader(erase_reply=b"IAP V1.2\r\nErase ok\r\n", ack_suffix=b"\r\n")
    img = IapImage.from_bytes(_image(PAGE_SIZE * 2 + 9))
    result = IapClient(boot).upgrade(img)
    assert result.checksum == img.checksum and "V1.2" in result.erase_reply


@pytest.mark.parametrize("size", [5000, 6144])
def test_identical_to_the_vendor_tool(size: int) -> None:
    """``vendor_tx_*`` is every byte IAP+Flash.exe wrote for ``synth_*`` (all bytes non-zero,
    so the padding is visible; 6144 = 3 * 2048 shows the extra zero page)."""
    data = (FIXTURES / f"synth_{size}.bin").read_bytes()
    assert IapImage.from_bytes(data).stream() == (FIXTURES / f"vendor_tx_synth_{size}.bin").read_bytes()


@pytest.mark.skipif(not VENDOR.is_file(), reason="vendor firmware not on this machine")
def test_vendor_image_matches_the_vendor_tool() -> None:
    data = VENDOR.read_bytes()
    img = IapImage.from_bytes(data)
    assert len(data) == 841_904 and img.last_page == 411 and len(img.pages) == 412
    assert img.erase_command().hex() == (b"Erase      ".hex() + "00000000" + "0000019b")
    # SHA-256 of the 843,795 bytes IAP+Flash.exe wrote for this file
    assert hashlib.sha256(img.stream()).hexdigest() == VENDOR_TX_SHA256
    assert img.checksum == 0x0498A486  # what the vendor tool showed as its checksum
