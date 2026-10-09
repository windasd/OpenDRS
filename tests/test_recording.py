"""Call-recording download: frames vs Wav.exe, list/read against a simulated radio, audio."""

from __future__ import annotations

import wave
from pathlib import Path

import pytest

from opendrs.recording import (
    BLOCK,
    DEFAULT_AMBEP2,
    RecordClient,
    RecordInfo,
    RecordingError,
    checksum16,
    decode_ambe,
    list_request,
    record_read_command,
    to_bit_stream,
    write_wav,
)
from opendrs.recording_sim import SimulatedRecorder, demo_records

# Every byte Wav.exe 1.00.0072 wrote while listing 3 records and downloading them,
# captured by running it against a simulated radio (legacy_probe/wav_harness).
WAV_EXE_TX = (Path(__file__).parent / "fixtures" / "recording" / "wav_exe_tx.bin").read_bytes()


def _client(radio: SimulatedRecorder) -> RecordClient:
    client = RecordClient(radio)
    client.LIST_TIMEOUT = client.REPLY_TIMEOUT = client.BLOCK_TIMEOUT = client.CHECKSUM_TIMEOUT = 0.05
    return client


def test_frames_match_wav_exe() -> None:
    assert list_request(100).hex(" ") == "68 58 ff 01 86 41 02 00 00 64 10"
    for n in (100, 99, 98, 97, 96):
        assert list_request(n) in WAV_EXE_TX
    assert record_read_command(1, 3) == b"Record Read\x00\x00\x00\x01\x00\x00\x00\x03"
    for number, info in enumerate(r[0] for r in demo_records()):
        assert record_read_command(info.number, info.pages_field) in WAV_EXE_TX, number
    assert checksum16(bytes.fromhex("6858ff010000020000641000")[:11]) == 0x8641


def test_record_line() -> None:
    info = RecordInfo.parse(b"junk#12#40#S#09998765#8433#2025/06/07 12:34Record Read")
    assert info == RecordInfo(12, 40, "S", 9998765, 8433, "2025/06/07 12:34")
    assert info.line() == b"#12#40#S#09998765#8433#2025/06/07 12:34"
    assert (info.blocks, info.pages_field, info.stem) == (3, 3, "20250607_1234_12_S09998765")
    assert round(info.seconds, 2) == 18.74
    exact = RecordInfo(1, 1, "R", 1, BLOCK, "2025/01/01 00:00")
    assert (exact.blocks, exact.pages_field) == (1, 2)  # Wav.exe's off-by-one, ignored by the radio
    assert RecordInfo.parse(b"#1#2#X#1#2#2025/01/01 00:00") is None


def test_list_and_read_against_simulated_radio() -> None:
    radio = SimulatedRecorder()
    client = _client(radio)
    infos = client.list_records()
    assert [i.number for i in infos] == [1, 2, 3] and infos == [r[0] for r in radio.records]
    for info, data in radio.records:
        header, got = client.read_record(info.number, info.pages_field)
        assert header == info and got == data


def test_list_stops_at_max_and_reports_unsupported_firmware() -> None:
    assert len(_client(SimulatedRecorder()).list_records(max_count=2)) == 2
    with pytest.raises(RecordingError, match="不支援從電腦讀取錄音"):
        _client(SimulatedRecorder(supports_list=False)).list_records()


def test_read_works_without_the_list_command() -> None:
    radio = SimulatedRecorder(supports_list=False)
    header, data = _client(radio).read_record(1)
    assert header.total == 3 and data == radio.records[0][1]


@pytest.mark.parametrize("radio, text", [
    (SimulatedRecorder(supports_record_read=False), "沒有回傳第 1 筆"),
    (SimulatedRecorder(corrupt_checksum=True), "校驗和不符"),
])
def test_read_failures(radio: SimulatedRecorder, text: str) -> None:
    with pytest.raises(RecordingError, match=text):
        _client(radio).read_record(1)


def test_missing_record_is_reported() -> None:
    with pytest.raises(RecordingError, match="沒有回傳第 9 筆"):
        _client(SimulatedRecorder()).read_record(9)


def test_bit_stream_is_msb_first_whole_frames() -> None:
    bits = to_bit_stream(bytes([0x80, 0x01]) + bytes(7) + b"\xff")
    assert len(bits) == 72 and bits[:16] == bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])


def test_write_wav(tmp_path: Path) -> None:
    pcm = (1000).to_bytes(2, "little", signed=True) * 160
    with wave.open(str(write_wav(tmp_path / "a.wav", pcm)), "rb") as w:
        assert (w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()) == (1, 2, 8000, 160)


def test_decode_reports_missing_ambep2(tmp_path: Path) -> None:
    with pytest.raises(RecordingError, match="找不到 ambep2"):
        decode_ambe(b"x" * 9, tmp_path / "ambep2.exe")


@pytest.mark.skipif(not DEFAULT_AMBEP2.is_file(), reason="vendor ambep2.exe not installed")
def test_decode_with_vendor_ambep2() -> None:
    data = demo_records()[0][1]
    pcm = decode_ambe(data, DEFAULT_AMBEP2)
    assert len(pcm) == len(data) // 9 * 320  # 160 samples of 16 bit per 9-byte frame


def test_bit_stream_matches_wav_exe() -> None:
    """``wav_exe_block.bit`` is what Wav.exe wrote for one 4050-byte block (demo record 2:
    3000 bytes of data + 0xFF padding) before handing it to ambep2."""
    info, data = demo_records()[1]
    block = data + b"\xff" * (BLOCK - info.length)
    expected = (Path(__file__).parent / "fixtures" / "recording" / "wav_exe_block.bit").read_bytes()
    assert to_bit_stream(block) == expected


class Chunked:
    """Hands the simulated radio's replies out a few bytes at a time (with empty reads)."""

    def __init__(self, inner: SimulatedRecorder, sizes: tuple[int, ...] = (1, 0, 7, 37, 3, 0, 13)) -> None:
        self.inner, self.sizes, self.i, self.buf = inner, sizes, 0, bytearray()

    def write(self, data: bytes) -> None:
        self.inner.write(data)

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        self.buf += self.inner.read_idle(idle, max_wait)
        n = self.sizes[self.i % len(self.sizes)]
        self.i += 1
        out = bytes(self.buf[:n])
        del self.buf[:n]
        return out

    def reset_input(self) -> None:
        self.inner.reset_input()
        self.buf.clear()

    def close(self) -> None:
        pass


def test_replies_split_into_small_reads() -> None:
    radio = SimulatedRecorder()
    client = RecordClient(Chunked(radio))
    client.BLOCK_TIMEOUT = 30.0
    assert client.list_records() == [r[0] for r in radio.records]
    header, data = client.read_record(1, 3)
    assert header.number == 1 and data == radio.records[0][1]


def test_cancel_list_and_read() -> None:
    with pytest.raises(RecordingError, match="已停止"):
        RecordClient(SimulatedRecorder(), cancelled=lambda: True).list_records()
    radio = SimulatedRecorder()
    calls = iter([False, True])
    with pytest.raises(RecordingError, match="已停止"):
        RecordClient(radio, cancelled=lambda: next(calls)).read_record(1, 3)


def test_simulated_radio_lists_again_from_the_top() -> None:
    client = _client(SimulatedRecorder())
    assert len(client.list_records(max_count=100)) == 3
    assert [i.number for i in client.list_records(max_count=2)] == [1, 2]


def test_decode_failure_is_reported(tmp_path: Path, monkeypatch) -> None:
    import subprocess

    exe = tmp_path / "ambep2.exe"
    exe.write_bytes(b"MZ")

    def failing(*args, **kwargs):  # noqa: ANN002, ANN003, ANN202
        raise subprocess.CalledProcessError(1, args[0], b"", b"header\nCan't open input data file rec.bit")

    monkeypatch.setattr(subprocess, "run", failing)
    with pytest.raises(RecordingError, match="結束碼 1.*Can't open input"):
        decode_ambe(b"x" * 9, exe)

    def no_output(*args, **kwargs):  # noqa: ANN002, ANN003, ANN202
        return subprocess.CompletedProcess(args[0], 0, b"", b"")

    monkeypatch.setattr(subprocess, "run", no_output)
    with pytest.raises(RecordingError, match="沒有產生輸出"):
        decode_ambe(b"x" * 9, exe)


def test_list_asks_for_record_numbers_like_the_firmware_handler() -> None:
    """The firmware's handler (unitA 0x030272c4) answers the record whose number is in the
    payload, and accepts a frame whose one's-complement sum over all bytes is 0."""
    radio = SimulatedRecorder()
    _client(radio).list_records()
    frames = [bytes(radio.sent[i:i + 11]) for i in range(0, len(radio.sent), 11)]
    assert frames == [list_request(n) for n in (1, 2, 3)]
    assert all(checksum16(f) == 0 for f in frames)
