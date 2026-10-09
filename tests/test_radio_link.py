"""Radio protocol: replay of the real capture, simulated radio, serial transport."""

from __future__ import annotations

import struct
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.codeplug import BLOCK, Counts, build_stream, parse_stream
from opendrs.contact_import import NewContact, append_contacts
from opendrs.radio_link import (
    RadioClient,
    RadioError,
    RadioIdentity,
    RadioTimeout,
    SerialTransport,
    command,
    list_ports,
)
from opendrs.radio_sim import SimulatedRadio
from opendrs.records import Contact
from opendrs.textcodec import NameCodec

CAP = Path(__file__).parent / "fixtures" / "capture"


def cap(name: str) -> bytes:
    return (CAP / name).read_bytes()


class ReplayTransport:
    """Plays back the radio side of the capture and checks every byte the PC sends."""

    def __init__(self, exchanges: list[tuple[bytes, bytes]]) -> None:
        self._ex = list(exchanges)
        self._pending = bytearray()
        self._out = bytearray()
        self.sent = bytearray()

    def write(self, data: bytes) -> None:
        self.sent += data
        self._pending += data
        while self._ex and len(self._pending) >= len(self._ex[0][0]):
            expected, response = self._ex.pop(0)
            got = bytes(self._pending[:len(expected)])
            assert got == expected, f"PC sent {got[:32]!r}, capture has {expected[:32]!r}"
            del self._pending[:len(expected)]
            self._out += response

    def read_exact(self, n: int, timeout: float) -> bytes:
        if len(self._out) < n:
            raise RadioTimeout("replay exhausted")
        data = bytes(self._out[:n])
        del self._out[:n]
        return data

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        data = bytes(self._out)
        self._out.clear()
        return data

    def reset_input(self) -> None:
        self._out.clear()

    def close(self) -> None:
        pass

    def finished(self) -> bool:
        return not self._ex and not self._pending


def read_exchanges() -> list[tuple[bytes, bytes]]:
    stream = cap("read_stream.bin")
    blocks = [stream[i:i + BLOCK] for i in range(0, len(stream), BLOCK)]
    ex = [(cap("read_cmd.bin"), cap("read_reply.bin"))]
    ex += [(b"Read", b) for b in blocks]
    ex[-1] = (b"Read", blocks[-1] + cap("read_checksum.bin"))
    return ex


def write_exchanges() -> list[tuple[bytes, bytes]]:
    stream = cap("write_stream.bin")
    tail = cap("write_radio_tail.bin")
    acks = tail.count(b"Write", 0, 30)
    blocks = [stream[i:i + BLOCK] for i in range(0, len(stream), BLOCK)]
    ex = [(cap("write_cmd.bin"), cap("write_reply.bin"))]
    ex += [(b, b"Write") for b in blocks[:-1]]
    ex.append((blocks[-1], tail[5 * acks:]))
    return ex


# ------------------------------------------------------------------ capture replay


def test_replay_read_matches_legacy_cps_byte_for_byte() -> None:
    t = ReplayTransport(read_exchanges())
    progress: list[tuple[int, int, str]] = []
    result = RadioClient(t, lambda d, n, s: progress.append((d, n, s))).read_codeplug()
    assert t.finished()
    assert result.counts == Counts(4, 27, 360)
    assert result.stream == cap("read_stream.bin")
    assert result.identity.model == "DRS-8500"
    assert result.identity.band == "136-480"
    assert result.identity.firmware.startswith("308E.BBKTARSCB.003")
    assert "DRS-8500" in result.identity.summary()
    assert progress[-1] == (7, 7, "讀取完成")
    written = f136.load(CAP / "written_project.F136")
    assert result.project.channels == written.channels


def test_replay_write_matches_legacy_cps_byte_for_byte() -> None:
    t = ReplayTransport(write_exchanges())
    project = f136.load(CAP / "written_project.F136")
    result = RadioClient(t).write_codeplug(project, date_text="10/8/2026 16:54")
    assert t.finished()
    assert bytes(t.sent) == cap("write_cmd.bin") + cap("write_stream.bin")
    assert result.checksum == sum(cap("write_stream.bin"))
    assert "Manufacture:TC58" in result.radio_log


# ------------------------------------------------------------------ simulated radio


@pytest.fixture
def radio() -> SimulatedRadio:
    return SimulatedRadio.with_stream(cap("read_stream.bin"), Counts(4, 27, 360))


def test_sim_read(radio: SimulatedRadio) -> None:
    result = RadioClient(radio).read_codeplug()
    assert result.stream == cap("read_stream.bin")
    assert result.identity.model == "DRS-8500"


def test_sim_write_then_read_back(radio: SimulatedRadio) -> None:
    client = RadioClient(radio)
    project = client.read_codeplug().project
    edited = project.with_channel(0, project.channels[0].with_name("NEW", NameCodec.ASCII))
    client.write_codeplug(edited, date_text="2026/10/08 18:00")
    assert radio.commits == 1
    back = client.read_codeplug()
    assert back.project.channels[0].name(NameCodec.ASCII) == "NEW"
    assert back.stream[0x60:0x70].rstrip(b"\x00") == b"2026/10/08 18:00"
    assert back.project.contacts == project.contacts


def test_sim_write_grows_part1_for_more_contacts(radio: SimulatedRadio) -> None:
    client = RadioClient(radio)
    project = client.read_codeplug().project
    extra = [NewContact(Contact.PRIVATE_CALL, f"BX{i}", 4670000 + i) for i in range(369)]
    big, _ = append_contacts(project, extra, NameCodec.ASCII)
    client.write_codeplug(big, date_text=None)
    back = client.read_codeplug()
    assert back.counts == Counts(4, 27, 729) and back.counts.part1_blocks == 9
    assert back.project.contacts == big.contacts
    assert back.project.zones == big.zones and back.project.channels == big.channels


def test_identity_veto_sends_nothing(radio: SimulatedRadio) -> None:
    client = RadioClient(radio)
    project = client.read_codeplug().project
    with pytest.raises(RadioError, match="未寫入任何資料：機型不符"):
        client.write_codeplug(project, None, expect=lambda ident: "機型不符")
    assert radio.commits == 0


def test_missing_ack_aborts_without_commit(radio: SimulatedRadio) -> None:
    client = RadioClient(radio)
    project = client.read_codeplug().project
    radio.stop_acking_after = 2
    with pytest.raises(RadioError, match="第 3 塊未獲確認"):
        client.write_codeplug(project, None)
    assert radio.commits == 0


def test_bad_write_checksum_is_reported(radio: SimulatedRadio) -> None:
    client = RadioClient(radio)
    project = client.read_codeplug().project
    radio.corrupt_write_checksum = True
    with pytest.raises(RadioError, match="校驗和不符"):
        client.write_codeplug(project, None)


def test_bad_read_checksum_is_reported(radio: SimulatedRadio, monkeypatch: pytest.MonkeyPatch) -> None:
    original = SimulatedRadio._send_block

    def corrupt(self: SimulatedRadio) -> None:
        before = len(self._out)
        original(self)
        if before < len(self._out):
            self._out[before] ^= 0x01  # flip a bit after the sum was taken

    monkeypatch.setattr(SimulatedRadio, "_send_block", corrupt)
    with pytest.raises(RadioError, match="校驗和不符"):
        RadioClient(radio).read_codeplug()


def test_unwritable_project_is_refused_before_connecting(radio: SimulatedRadio) -> None:
    with pytest.raises(RadioError, match="無法寫入"):
        RadioClient(radio).write_codeplug(f136.Project(), None)
    assert radio.commits == 0


class Garbage:
    def __init__(self, reply: bytes) -> None:
        self._reply = reply

    def write(self, data: bytes) -> None:
        pass

    def read_exact(self, n: int, timeout: float) -> bytes:
        if len(self._reply) < n:
            raise RadioTimeout("x")
        data, self._reply = self._reply[:n], self._reply[n:]
        return data

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        return b""

    def reset_input(self) -> None:
        pass

    def close(self) -> None:
        pass


def test_wrong_reply_prefix() -> None:
    with pytest.raises(RadioError, match="回應不符"):
        RadioClient(Garbage(b"X" * 40)).read_codeplug()
    with pytest.raises(RadioError, match="回應不符"):
        RadioClient(Garbage(b"X" * 40)).write_codeplug(
            f136.load(CAP / "written_project.F136"), None)


def test_inconsistent_block_counts_in_reply() -> None:
    reply = b"  Read_2M_\x00\x00" + struct.pack("<5H", 4, 27, 360, 5, 1)
    with pytest.raises(RadioError, match="區塊數"):
        RadioClient(Garbage(reply)).read_codeplug()


def test_no_reply_times_out() -> None:
    with pytest.raises(RadioTimeout):
        RadioClient(Garbage(b"")).read_codeplug()


def test_missing_checksum_tag() -> None:
    reply = b"  Read_2M_\x00\x00" + struct.pack("<5H", 1, 1, 0, 3, 1)
    stream = parse_stream  # noqa: F841 (keeps import used in all paths)
    body = bytes(BLOCK) * 4
    with pytest.raises(RadioError, match="校驗碼"):
        RadioClient(Garbage(reply + body + b"NOPE" + bytes(9))).read_codeplug()


# ------------------------------------------------------------------ helpers / transport


def test_command_layout() -> None:
    assert command(b"Flash Read ", 6) == cap("read_cmd.bin")
    assert command(b"Flash Write", 7) == cap("write_cmd.bin")
    cmd = command(b"Flash Write", 7, b"123456", b"9")
    assert cmd[19:25] == b"123456" and cmd[25:31] == b"9" + bytes(5) and len(cmd) == 31
    with pytest.raises(ValueError):
        command(b"Flash Read ", 1, b"1234567")


def test_identity_parsing_variants() -> None:
    ident = RadioIdentity.parse(cap("read_reply.bin")[22:])
    assert (ident.model, ident.band) == ("DRS-8500", "136-480")
    blank = RadioIdentity.parse(b"\x00\x00")
    assert blank.model == "" and "未知機型" in blank.summary()


def test_list_ports_prefers_cable(monkeypatch: pytest.MonkeyPatch) -> None:
    class P:
        def __init__(self, dev: str, desc: str, vid: int | None) -> None:
            self.device, self.description, self.vid = dev, desc, vid

    import serial.tools.list_ports as lp

    monkeypatch.setattr(lp, "comports", lambda: [P("COM1", "Communications Port", None),
                                                 P("COM4", "Prolific PL2303GC", 0x067B)])
    ports = list_ports()
    assert ports[0].device == "COM4" and ports[0].is_programming_cable
    assert not ports[1].is_programming_cable


def test_serial_transport_loopback() -> None:
    t = SerialTransport("loop://")
    try:
        t.write(b"Read")
        assert t.read_exact(4, 1.0) == b"Read"
        t.write(b"abc")
        assert t.read_idle(0.1, 1.0) == b"abc"
        with pytest.raises(RadioTimeout):
            t.read_exact(1, 0.2)
        t.write(b"zz")
        t.reset_input()
        assert t.read_idle(0.05, 0.2) == b""
    finally:
        t.close()


def test_serial_open_failure() -> None:
    with pytest.raises(RadioError, match="無法開啟"):
        SerialTransport("COM_DOES_NOT_EXIST_99")


def test_build_stream_used_by_client_equals_codeplug(radio: SimulatedRadio) -> None:
    project = f136.load(CAP / "written_project.F136")
    assert build_stream(project, "10/8/2026 16:54") == cap("write_stream.bin")


def test_silent_radio_gets_a_checklist() -> None:
    from opendrs.radio_link import NO_REPLY, RadioTimeout

    class Silent:
        def reset_input(self) -> None: ...
        def write(self, data: bytes) -> None: ...
        def read_exact(self, n: int, timeout: float) -> bytes:
            raise RadioTimeout("x", 0)
        def close(self) -> None: ...

    with pytest.raises(RadioTimeout) as info:
        RadioClient(Silent()).read_codeplug()
    assert str(info.value) == NO_REPLY and "開機" in NO_REPLY
