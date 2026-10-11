"""Export what the Python firmware tools compute, as the oracle for the Rust port (opendrs-firmware):
IAP images and upgrades against the simulated bootloader, recording frames / record lines / list and
download runs against the simulated recorder, firmware checks and the record-read patch.

Run from the repository root with the Python package importable:
    PYTHONPATH=../opendrs-oracle/src python rust/tools/make_parity_firmware.py > rust/opendrs-firmware/tests/parity.json

Scenarios are described as data (image size, simulator settings, transport, cancel pattern, ...) so
the Rust test can rebuild each one; large byte strings are exported as SHA-256 only.
"""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
from itertools import product
from pathlib import Path

from opendrs import firmware_check, fwpatch, hardware, iap, recording
from opendrs.firmware_check import FirmwareFile
from opendrs.iap import PAGE_SIZE, IapClient, IapError, IapImage
from opendrs.iap_sim import SimulatedBootloader
from opendrs.radio_link import RadioError, RadioTimeout
from opendrs.recording import RecordClient, RecordInfo, RecordingError
from opendrs.recording_sim import SimulatedRecorder, demo_records

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "rust" / "fixtures"
FAST = 0.05  # reply timeouts for runs that are expected to wait for nothing


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def pattern(size: int, seed: int = 3) -> bytes:
    """The test suite's synthetic image: (i * 7 + seed) & 0xFF."""
    return bytes((i * 7 + seed) & 0xFF for i in range(size))


def xorshift(size: int, seed: int) -> bytes:
    x, out = seed, bytearray(size)
    for i in range(size):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        out[i] = x & 0xFF
    return bytes(out)


def bytes_facts(data: bytes) -> dict:
    out = {"len": len(data), "sha256": sha(data)}
    if len(data) <= 256:
        out["hex"] = data.hex()
    return out


class Cancel:
    """Answers from ``answers`` in turn, then keeps repeating the last one (never: [False])."""

    def __init__(self, answers: list[bool]) -> None:
        self.answers, self.calls = list(answers), 0

    def __call__(self) -> bool:
        answer = self.answers[min(self.calls, len(self.answers) - 1)]
        self.calls += 1
        return answer


# ---------------------------------------------------------------------- IAP


def image_facts(img: IapImage) -> dict:
    return {"size": img.size, "address": img.address, "last_page": img.last_page, "pages": len(img.pages),
            "checksum": img.checksum, "erase_command": img.erase_command().hex(),
            "last_page_sha256": sha(img.pages[-1]), "stream_sha256": sha(img.stream())}


def iap_images() -> dict:
    fixtures = {}
    for size in (5000, 6144):
        data = (FIX / "iap" / f"synth_{size}.bin").read_bytes()
        facts = image_facts(IapImage.from_bytes(data))
        facts["vendor_tx_sha256"] = sha((FIX / "iap" / f"vendor_tx_synth_{size}.bin").read_bytes())
        facts["file_sha256"] = sha(data)
        fixtures[f"synth_{size}.bin"] = facts
    synthetic = []
    for size, address, pad in [(1, 0, 0), (100, 0, 0), (2047, 0, 0), (2048, 0, 0), (2049, 0, 0),
                               (PAGE_SIZE * 2 + 5, 0, 0), (PAGE_SIZE * 3, 0, 0), (PAGE_SIZE * 5 + 1234, 0x4000, 0),
                               (5000, 0xFFFFFFFF, 0xFF), (4 * 1024 * 1024, 0, 0)]:
        img = IapImage.from_bytes(pattern(size), address=address, pad=pad)
        synthetic.append({"size": size, "address": address, "pad": pad, "facts": image_facts(img)})
    errors = []
    for size, address in [(0, 0), (4 * 1024 * 1024 + 1, 0), (1, -1), (1, 2 ** 32), (12_345_678, 0)]:
        try:
            IapImage.from_bytes(pattern(size), address=address)
            errors.append({"size": size, "address": address, "error": None})
        except ValueError as exc:
            errors.append({"size": size, "address": address, "error": str(exc)})
    return {"fixtures": fixtures, "synthetic": synthetic, "errors": errors}


class ScriptedTransport:
    """Hands out ``chunks`` one per read (b"" = an empty read, like a quiet serial port)."""

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


def boot_spec(**changes) -> dict:  # noqa: ANN003
    spec = {"erase_reply": b"IAP Erase ok".hex(), "ack_suffix": "", "silent": False, "stop_acking_after": None,
            "bad_ack": None, "corrupt_checksum": False}
    spec.update(changes)
    return spec


def make_boot(spec: dict) -> SimulatedBootloader:
    return SimulatedBootloader(erase_reply=bytes.fromhex(spec["erase_reply"]),
                               ack_suffix=bytes.fromhex(spec["ack_suffix"]), silent=spec["silent"],
                               stop_acking_after=spec["stop_acking_after"],
                               bad_ack=None if spec["bad_ack"] is None else bytes.fromhex(spec["bad_ack"]),
                               corrupt_checksum=spec["corrupt_checksum"])


def run_upgrade(transport, img: IapImage, cancelled, erase_timeout: float | None) -> dict:  # noqa: ANN001
    steps: list[list] = []
    client = IapClient(transport, lambda d, t, s: steps.append([d, t, s]), cancelled)
    if erase_timeout is not None:
        client.ERASE_TIMEOUT = erase_timeout
    out: dict = {"result": None, "error": None}
    try:
        r = client.upgrade(img)
        out["result"] = {"checksum": r.checksum, "pages": r.pages, "erase_reply": r.erase_reply}
    except IapError as exc:
        out["error"] = {"message": str(exc), "erase_sent": exc.erase_sent}
    out["steps"] = steps
    out["client_erase_sent"] = client.erase_sent
    return out


def iap_scenarios() -> list:
    small, three, five = PAGE_SIZE + 3, PAGE_SIZE * 3 + 1, PAGE_SIZE * 5 + 1234
    boot_cases = [
        ("full_upgrade", five, 0x4000, boot_spec(), {"answers": [False]}),
        ("extra_text", PAGE_SIZE * 2 + 9, 0, boot_spec(erase_reply=b"IAP V1.2\r\nErase ok\r\n".hex(),
                                                       ack_suffix=b"\r\n".hex()), {"answers": [False]}),
        ("upper_case_ok_non_ascii", 100, 0, boot_spec(erase_reply=b"\xffIAP ERASE OK\x80".hex()), {"answers": [False]}),
        ("exact_pages", PAGE_SIZE * 2, 0, boot_spec(), {"answers": [False]}),
        ("silent", 100, 0, boot_spec(silent=True), {"answers": [False]}),
        ("stop_acking_after_2", three, 0, boot_spec(stop_acking_after=2), {"answers": [False]}),
        ("bad_ack", three, 0, boot_spec(bad_ack=b"err".hex()), {"answers": [False]}),
        ("empty_bad_ack_means_kyd", three, 0, boot_spec(bad_ack=""), {"answers": [False]}),
        ("corrupt_checksum", three, 0, boot_spec(corrupt_checksum=True), {"answers": [False]}),
        ("cancel_before_erase", 100, 0, boot_spec(), {"answers": [True]}),
        ("cancel_between_pages", three, 0, boot_spec(), {"after_flash": 2 * PAGE_SIZE}),
        ("cancel_on_third_call", three, 0, boot_spec(), {"answers": [False, False, True]}),
    ]
    out = []
    for name, size, address, spec, cancel in boot_cases:
        img = IapImage.from_bytes(pattern(size), address=address)
        boot = make_boot(spec)
        if "after_flash" in cancel:
            limit = cancel["after_flash"]
            cancelled = lambda boot=boot, limit=limit: len(boot.flash) >= limit  # noqa: E731
        else:
            cancelled = Cancel(cancel["answers"])
        run = run_upgrade(boot, img, cancelled, None)
        run["boot"] = {"sent": bytes_facts(bytes(boot.sent)), "flash": bytes_facts(bytes(boot.flash)),
                       "address": boot.address, "last_page": boot.last_page, "erased": boot.erased,
                       "finished": boot.finished}
        out.append({"name": name, "size": size, "address": address, "transport": {"boot": spec},
                    "cancel": cancel, "erase_timeout": None, "run": run})

    def checksum_reply(img: IapImage) -> list[bytes]:
        tail = b"ChecksumI" + img.checksum.to_bytes(4, "big")
        return [tail[:5], b"", tail[5:11], tail[11:]]

    img_small = IapImage.from_bytes(pattern(small))
    scripted = [
        ("split_replies", small, [b"", b"IAP Erase o", b"", b"k\r\n", b"k", b"yd"] + checksum_reply(img_small),
         None, False, [False], None),
        ("checksum_only_after_last_page", small,
         [b"ok", b"kyd", b"ChecksumI" + img_small.checksum.to_bytes(4, "big")], None, False, [False], None),
        ("checksum_tag_lower_case", small,
         [b"OK", b"KYD", b"checksumi" + img_small.checksum.to_bytes(4, "big")], None, False, [False], None),
        ("silence_after_erase", 100, [], None, False, [False], 0.05),
        ("garbage_after_erase", 100, [b"IAP Erase", b"x" * 50 + b"\x00'\\"], None, False, [False], 0.2),
        ("cancel_while_waiting_for_erase", 100, [], None, False, [False, False, False, True], None),
        ("reset_fails", PAGE_SIZE * 3, [b"ok", b"kyd", b"kyd"], None, True, [False], None),
        ("erase_write_fails", PAGE_SIZE * 3, [b"ok", b"kyd", b"kyd"], 0, False, [False], None),
        ("page_write_fails", PAGE_SIZE * 3, [b"ok", b"kyd", b"kyd"], 2, False, [False], None),
    ]
    for name, size, chunks, fail_on_write, fail_reset, answers, erase_timeout in scripted:
        img = IapImage.from_bytes(pattern(size))
        t = ScriptedTransport(chunks, fail_on_write=fail_on_write, fail_reset=fail_reset)
        run = run_upgrade(t, img, Cancel(answers), erase_timeout)
        run["scripted"] = {"sent": bytes_facts(b"".join(t.sent)), "writes": len(t.sent)}
        out.append({"name": name, "size": size, "address": 0,
                    "transport": {"script": {"chunks": [c.hex() for c in chunks], "fail_on_write": fail_on_write,
                                             "fail_reset": fail_reset}},
                    "cancel": {"answers": answers}, "erase_timeout": erase_timeout, "run": run})
    return out


# ---------------------------------------------------------------------- recording


def info_facts(info: RecordInfo | None) -> dict | None:
    if info is None:
        return None
    return {"number": info.number, "total": info.total, "direction": info.direction, "call_id": info.call_id,
            "length": info.length, "time": info.time, "line": info.line().hex(), "blocks": info.blocks,
            "pages_field": info.pages_field, "seconds": info.seconds, "stem": info.stem}


def record_lines() -> list:
    texts = [b"junk#12#40#S#09998765#8433#2025/06/07 12:34Record Read", b"#1#2#X#1#2#2025/01/01 00:00",
             b"#1#1#R#1#4050#2025/01/01 00:00", b"#0#0#R#0#0#2025/01/01 00:00", b"#3#3#R#00000009#4050#2025/06/09 23:59",
             b"#1#3#R#12345678901#5#2025/01/01 00:00", b"#1#3#R#1234567890#5#2025/01/01 00:00",
             b"##1#3#R#1#5#2025/01/01 00:00", b"#1#3#R#1#5#2025/1/01 00:00", b"#1#3#S#1#4051#2025/01/01 00:00",
             b"#18446744073709551615#1#R#1#1#2025/01/01 00:00", b"\xff\xfe#2#3#S#00012345#3000#2025/06/08 08:05\x00",
             b"#7#9#R#42#1#2025/12/31 23:59#8#9#S#43#2#2026/01/01 00:00", b"#1#1#R#1#8100#2025/01/01 00:0",
             b"#1#1#R#1#8101#2025/01/01 00:00", b"#1#1#R#99999999#12150#1999/09/09 09:09", b"",
             b"#1#1#R##5#2025/01/01 00:00", b"#01#002#S#0#000#2025/01/01 00:00"]
    return [{"text": t.hex(), "info": info_facts(RecordInfo.parse(t))} for t in texts]


def records_spec(records: list[tuple[RecordInfo, bytes]]) -> list:
    return [{"info": [i.number, i.total, i.direction, i.call_id, i.length, i.time], "data": data.hex()}
            for i, data in records]


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

    def read_exact(self, n: int, timeout: float) -> bytes:
        raise AssertionError("not used")

    def reset_input(self) -> None:
        self.inner.reset_input()
        self.buf.clear()

    def close(self) -> None:
        pass


class Truncated:
    """Passes at most ``limit`` reply bytes through, then stays silent."""

    def __init__(self, inner: SimulatedRecorder, limit: int) -> None:
        self.inner, self.left = inner, limit

    def write(self, data: bytes) -> None:
        self.inner.write(data)

    def read_idle(self, idle: float, max_wait: float) -> bytes:
        data = self.inner.read_idle(idle, max_wait)[:self.left]
        self.left -= len(data)
        return data

    def read_exact(self, n: int, timeout: float) -> bytes:
        raise AssertionError("not used")

    def reset_input(self) -> None:
        self.inner.reset_input()

    def close(self) -> None:
        pass


def error_facts(exc: Exception) -> dict:
    if isinstance(exc, RecordingError):
        return {"type": "RecordingError", "message": str(exc)}
    if isinstance(exc, RadioTimeout):
        return {"type": "RadioTimeout", "message": str(exc), "received": exc.received}
    assert isinstance(exc, RadioError)
    return {"type": "RadioError", "message": str(exc)}


def run_ops(client: RecordClient, radio: SimulatedRecorder, ops: list) -> list:
    results = []
    for op in ops:
        try:
            if op[0] == "list":
                infos = client.list_records(op[1])
                results.append({"ok": [info_facts(i) for i in infos], "list_stopped": client.list_stopped})
            elif op[0] == "read":
                header, data = client.read_record(op[1], op[2])
                results.append({"ok": {"header": info_facts(header), "data": bytes_facts(data)}})
            else:  # raw bytes straight to the simulated radio
                radio.write(bytes.fromhex(op[1]))
                results.append({"ok": None})
        except RadioError as exc:
            results.append({"error": error_facts(exc)})
    return results


def recording_scenarios() -> list:
    demo = demo_records()
    line1, line2 = len(demo[0][0].line()), len(demo[1][0].line())
    odd = [(RecordInfo(5, 1, "R", 1, 100, "2025/01/01 00:00"), pattern(5000, 11))]
    short = [(RecordInfo(1, 2, "S", 77, 8000, "2025/02/02 02:02"), pattern(10, 5)),
             (RecordInfo(2, 2, "R", 78, 0, "2025/02/02 02:03"), b"")]
    bad_frame = bytes.fromhex("6858ff0100000200000110")
    reads = [["read", i.number, i.pages_field] for i, _ in demo]
    cases = [
        ("demo_list_read", None, {}, "direct", [False], [["list", 100]] + reads),
        ("list_max_2", None, {}, "direct", [False], [["list", 2]]),
        ("list_max_0", None, {}, "direct", [False], [["list", 0]]),
        ("list_max_1000_twice", None, {}, "direct", [False], [["list", 1000], ["list", 2]]),
        ("list_unsupported", None, {"supports_list": False}, "direct", [False], [["list", 100]]),
        ("read_without_list", None, {"supports_list": False}, "direct", [False], [["read", 1, 1]]),
        ("read_unsupported", None, {"supports_record_read": False}, "direct", [False], [["read", 1, 1]]),
        ("read_corrupt", None, {"corrupt_checksum": True}, "direct", [False], [["read", 1, 1]]),
        ("read_missing_and_zero", None, {}, "direct", [False], [["read", 9, 1], ["read", 0, 1], ["read", 2, 1]]),
        ("chunked", None, {}, "chunked", [False], [["list", 100], ["read", 1, 3]]),
        ("cancel_list", None, {}, "direct", [True], [["list", 100]]),
        ("cancel_read", None, {}, "direct", [False, True], [["read", 1, 3]]),
        ("cancel_list_midway", None, {}, "direct", [False, False, True], [["list", 100]]),
        ("mismatched_number", odd, {}, "direct", [False], [["list", 100], ["read", 1, 1]]),
        ("data_longer_and_shorter_than_length", short, {}, "direct", [False],
         [["list", 100], ["read", 1, 1], ["read", 2, 1]]),
        ("truncated_block", None, {}, {"truncate": line1 + 11 + 1000}, [False], [["read", 1, 3]]),
        ("truncated_no_verb", None, {}, {"truncate": line1}, [False], [["read", 1, 3]]),
        ("truncated_no_checksum", None, {}, {"truncate": line2 + 11 + 4050}, [False], [["read", 2, 1]]),
        ("noise_then_list", None, {}, "direct", [False],
         [["write", b"xyz".hex()], ["list", 100], ["write", bad_frame.hex()], ["list", 1],
          ["write", b"Rec".hex()], ["list", 100]]),
        ("short_frames", None, {}, "direct", [False],
         [["write", "6858ff01000000001068"], ["write", "58ff0100000100011068"], ["list", 3]]),
    ]
    out = []
    for name, records, flags, transport, answers, ops in cases:
        radio = SimulatedRecorder() if records is None else SimulatedRecorder(records=list(records))
        for key, value in flags.items():
            setattr(radio, key, value)
        if transport == "direct":
            t = radio
        elif transport == "chunked":
            t = Chunked(radio)
        else:
            t = Truncated(radio, transport["truncate"])
        steps: list[list] = []
        client = RecordClient(t, lambda d, n, s: steps.append([d, n, s]), Cancel(answers))
        if transport == "chunked":
            client.BLOCK_TIMEOUT = 30.0
        else:
            client.LIST_TIMEOUT = client.REPLY_TIMEOUT = client.BLOCK_TIMEOUT = client.CHECKSUM_TIMEOUT = FAST
        results = run_ops(client, radio, ops)
        out.append({"name": name, "records": None if records is None else records_spec(records),
                    "flags": flags, "transport": transport, "cancel": {"answers": answers}, "ops": ops,
                    "results": results, "steps": steps, "sent": bytes_facts(bytes(radio.sent))})
    return out


def recording_section() -> dict:
    sums = ["", "00", "ff", "ffff", "6858ff01000002000064", "6858ff010000020000641000"[:22], "0102030405"]
    sums += [xorshift(n, seed).hex() for n, seed in ((1, 7), (2, 9), (3, 11), (100, 13), (1001, 17))]
    frames = [(0x58, "0064"), (0x58, ""), (0x21, "010203"), (0x88, "ff" * 10), (0x00, "00" * 300)]
    bit_inputs = ["", "8001" + "00" * 7 + "ff", "ff" * 9, "00" * 8, "a5" * 18, xorshift(37, 5).hex()]
    pcms = [(1000).to_bytes(2, "little", signed=True) * 160, b"\x01\x02\x03", b"", xorshift(1001, 3)]
    wavs = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, pcm in enumerate(pcms):
            path = recording.write_wav(Path(tmp) / f"{i}.wav", pcm)
            wavs.append({"pcm": pcm.hex(), "wav": bytes_facts(path.read_bytes())})
    demo = demo_records()
    block = demo[1][1] + b"\xff" * (recording.BLOCK - demo[1][0].length)
    return {
        "checksum16": [[h, recording.checksum16(bytes.fromhex(h))] for h in sums],
        "h_frame": [[cmd, payload, recording.h_frame(cmd, bytes.fromhex(payload)).hex()] for cmd, payload in frames],
        "list_request": [[n, recording.list_request(n).hex()] for n in (0, 1, 2, 3, 99, 100, 200, 65535)],
        "record_read_command": [[n, p, recording.record_read_command(n, p).hex()]
                                for n, p in ((1, 3), (0, 0), (255, 1), (4294967295, 4294967295), (9, 2))],
        "lines": record_lines(),
        "bit_stream": [[h, bytes_facts(recording.to_bit_stream(bytes.fromhex(h)))] for h in bit_inputs],
        "bit_stream_demo": [bytes_facts(recording.to_bit_stream(data)) for _, data in demo],
        "bit_stream_block": {"input_sha256": sha(block), "bits": bytes_facts(recording.to_bit_stream(block)),
                             "fixture_sha256": sha((FIX / "recording" / "wav_exe_block.bit").read_bytes())},
        "wav": wavs,
        "demo_records": [{"info": info_facts(i), "data": bytes_facts(d)} for i, d in demo],
        "scenarios": recording_scenarios(),
        "constants": {"block": recording.BLOCK, "frame_bytes": recording.FRAME_BYTES,
                      "sample_rate": recording.SAMPLE_RATE, "list_cmd": recording.LIST_CMD,
                      "default_list_max": recording.DEFAULT_LIST_MAX, "max_records": recording.MAX_RECORDS,
                      "record_verb": recording.RECORD_VERB.hex(), "checksum_read": recording.CHECKSUM_READ.hex(),
                      "default_ambep2": str(recording.DEFAULT_AMBEP2)},
    }


# ---------------------------------------------------------------------- firmware_check

IDS = ["308E.BBKTARSCB.003", "308E.BAxTARSCB.003", "308E.B6.TARSCB.003", "308E.B64TARSCB.003",
       "308E.BXYTARSCB.003", "308E.B6xTARSCB.003", "308E.CBKTARSCB.003", "308EBBKTARSCB", "", "1234.BAxQ",
       "308E.B", "308E.BB", "308E.BBK", "x.BBK", ".BBKz", "308E.bBKT", "頻道.BBK測試", "308E.B6.", "308E.BBKTARSCB.004",
       "412A.XX.TARSCB.001", "a.b.c", "308E.B頻K"]


def fw_image(firmware_id: bytes, date: bytes = b"Mar  9 2024") -> bytes:
    return b"\x00" * 64 + firmware_id + b"\x00" * 32 + date + b"\x0017:02:38" + b"\xff" * 64


def inspect_cases() -> list:
    images = [fw_image(b"308E.B6.TARSCB.003."), fw_image(b"no id here", b""), fw_image(b"308E.BBKTARSCB.003.", b"Dec 19 2023"),
              fw_image(b"412A.XX.TARSCB.001."), fw_image(b"308E.B6.TARSCB.003"), fw_image(b"1234.A.B.C.D.E.F.123."),
              fw_image(b"308e.B6.TARSCB.003."), fw_image(b"308E.B6.TARSCB.004.", b"Mar 16 2024"),
              b"Jan  1 2020\x00" + b"308E.B6.TARSCB.003." + b"\x00" * 8,
              fw_image(b"999F.Z1234.567.", b"Feb 29 2024"), fw_image(b"308E.B6.TOOLONGNAME1234567890ABC.003.", b"Mar  9 2024"),
              b"", b"Mar  9 2024 308E.B6.TARSCB.003.Sep 30 2025"]
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, data in enumerate(images):
            path = Path(tmp) / f"{i}.bin"
            path.write_bytes(data)
            img = firmware_check.inspect(path)
            out.append({"data": data.hex(), "firmware_id": img.firmware_id, "build_date": img.build_date,
                        "hardware": img.hardware, "size": img.size, "sha256": img.sha256})
        big = Path(tmp) / "big.bin"
        big.write_bytes(bytes(4 * 1024 * 1024 + 1))
        try:
            firmware_check.inspect(big)
            too_large = None
        except ValueError as exc:
            too_large = str(exc)
    return out, too_large


def check_cases() -> tuple[dict, list, list]:
    """Every combination below; finding texts are stored once in ``texts`` and referenced by index."""
    shas = {"plain": "0" * 64}
    for i, key in enumerate(firmware_check.KNOWN_VENDOR_IMAGES):
        shas[f"vendor{i}"] = key
    for i, key in enumerate(firmware_check.KNOWN_PATCHED_IMAGES):
        shas[f"patched{i}"] = key
    for i, key in enumerate(firmware_check.BLOCKED_IMAGES):
        shas[f"blocked{i}"] = key
    radios = ["308E.BBKTARSCB.003", "", "308E.B6.TARSCB.004", "308E.BAxTARSCB.003", "412A.XX.TARSCB.001"]
    radio_dates = ["Mar 9 2024", "", "Dec 19 2023"]
    ids = ["308E.BBKTARSCB.003", "308E.B6.TARSCB.003", "308E.B6.TARSCB.004", "308E.BXYTARSCB.003",
           "412A.XX.TARSCB.001", ""]
    combos = [("plain", i, d, b) for i, d, b in product(ids, ["Mar 9 2024", ""], [True, False])]
    combos += [("patched0", i, d, True) for i, d in product(ids[1:3], ["Mar 9 2024", ""])]
    combos += [(s, "308E.B6.TARSCB.003", "Mar 9 2024", True)
               for s in ("vendor0", "vendor1", "patched1", "patched2", "patched3", "blocked0", "blocked1")]
    out, texts = [], {}
    for (sha_key, file_id, date, backup), radio, radio_date in product(combos, radios, radio_dates):
        image = FirmwareFile(Path("fw.bin"), 1024, shas[sha_key], file_id, date)
        findings = firmware_check.check(image, radio, backup, radio_date)
        out.append([sha_key, file_id, date, backup, radio, radio_date,
                    [[f.level.value, texts.setdefault(f.text, len(texts))] for f in findings],
                    firmware_check.verdict(findings).value])
    return shas, out, list(texts)


def firmware_check_section() -> dict:
    inspected, too_large = inspect_cases()
    shas, checks, texts = check_cases()
    return {
        "ids": [{"id": i, "normalised": firmware_check.normalise_firmware_id(i), "rf_chip": firmware_check.rf_chip(i)}
                for i in IDS],
        "same_build": [[a, b, firmware_check.same_build(a, b)] for a, b in product(IDS, IDS)],
        "inspect": inspected,
        "too_large": too_large,
        "shas": shas,
        # [sha key, file id, file date, backup, radio id, radio date, [[level, text index]], verdict]
        "check": checks,
        "check_texts": texts,
        "verdict_empty": firmware_check.verdict([]).value,
        "tables": {"blocked": firmware_check.BLOCKED_IMAGES, "vendor": firmware_check.KNOWN_VENDOR_IMAGES,
                   "patched": {k: list(v) for k, v in firmware_check.KNOWN_PATCHED_IMAGES.items()},
                   "rf_chip_codes": firmware_check.RF_CHIP_CODES, "template": firmware_check.TEMPLATE_CHIP_CODE,
                   "max_image": firmware_check.MAX_IMAGE, "default_iap_exe": str(firmware_check.DEFAULT_IAP_EXE)},
    }


# ---------------------------------------------------------------------- fwpatch

SIZE = 841_904


def synthetic(extra: dict[int, bytes] | None = None, size: int = SIZE) -> bytes:
    img = bytearray(size)
    for site in fwpatch.RECORD_READ:
        o = site.va - fwpatch.A_BASE
        img[o:o + len(site.original)] = site.original
    for va, data in (extra or {}).items():
        o = va - fwpatch.A_BASE
        img[o:o + len(data)] = data
    return bytes(img)


def refs_facts(refs: list[str]) -> dict:
    return {"count": len(refs), "sha256": sha("\n".join(refs).encode()), "first": refs[:20], "last": refs[-5:]}


def patch_facts(data: bytes) -> dict:
    out = {"status": fwpatch.record_read_status(data), "is_patched": fwpatch.is_record_read_patched(data)}
    try:
        r = fwpatch.patch_record_read(data)
        out["result"] = {"sha256": r.sha256, "base_sha256": r.base_sha256, "changed": r.changed,
                         "base_name": r.base_name, "status_after": fwpatch.record_read_status(r.data),
                         "is_patched_after": fwpatch.is_record_read_patched(r.data)}
        out["error"] = None
    except fwpatch.PatchError as exc:
        out["result"] = None
        out["error"] = str(exc)
    return out


def bsr32(at: int, target: int) -> bytes:
    off = ((target - at) // 2) & 0x3FFFFFF
    w = 0xE000_0000 | off
    return (w >> 16).to_bytes(2, "little") + (w & 0xFFFF).to_bytes(2, "little")


def fwpatch_section() -> dict:
    target = 0x03027030
    br32_off = ((target - 0x03030010) // 2) & 0xFFFF
    extras = {
        "none": {},
        "bsr32": {0x03030000: bsr32(0x03030000, target)},
        "br32": {0x03030010: (0xE800).to_bytes(2, "little") + br32_off.to_bytes(2, "little")},
        "lrw32": {0x03027000: ((0b111010 << 10) | (0x14 << 5) | 3).to_bytes(2, "little") + (12).to_bytes(2, "little")},
        "br16": {0x03027000: ((1 << 10) | 0x18).to_bytes(2, "little")},
        "lrw16": {0x03026FF0: (0x1010).to_bytes(2, "little")},
        "pointer": {0x03040000: bytes.fromhex("02033070")},
        "pointer_swapped": {0x03040100: bytes.fromhex("30700203")},
        "six_refs": {0x03030000 + 16 * k: bsr32(0x03030000 + 16 * k, target) for k in range(6)},
        "inside_dead_code_only": {0x03027040: ((1 << 10) | 0x8).to_bytes(2, "little")},
    }
    cases = {name: patch_facts(synthetic(extra)) for name, extra in extras.items()}
    wrong = bytearray(synthetic())
    wrong[0x03026E5C - fwpatch.A_BASE] ^= 1
    cases["one_bit_wrong"] = patch_facts(bytes(wrong))
    cases["too_small_100"] = patch_facts(b"\0" * 100)
    cases["exactly_cut"] = patch_facts(synthetic(size=fwpatch.CUT))
    cases["cut_plus_one_zeros"] = patch_facts(bytes(fwpatch.CUT + 1))
    patched = fwpatch.patch_record_read(synthetic()).data
    cases["patched_again"] = patch_facts(patched)
    cases["patched_one_site_reverted"] = patch_facts(
        patched[:0x03027024 - fwpatch.A_BASE] + bytes.fromhex("d2142614") + patched[0x03027028 - fwpatch.A_BASE:])
    noise = xorshift(fwpatch.CUT + 0x4000, 0x1234567)
    refs = {
        "synthetic_cave": refs_facts(fwpatch.refs_into(synthetic(), fwpatch.CAVE_START, fwpatch.CAVE_END)),
        "patched_cave": refs_facts(fwpatch.refs_into(patched, fwpatch.CAVE_START, fwpatch.CAVE_END)),
        "patched_dead_code": refs_facts(fwpatch.refs_into(patched, *fwpatch.DEAD_CODE)),
        "noise_unit_a": refs_facts(fwpatch.refs_into(noise, 0x03000000, 0x03100000)),
        "noise_unit_b": refs_facts(fwpatch.refs_into(noise, 0x00010000, 0x00020000)),
        "noise_narrow": refs_facts(fwpatch.refs_into(noise, 0x03050000, 0x03050400)),
        "noise_odd_length": refs_facts(fwpatch.refs_into(noise[:4097], 0x03000000, 0x03002000)),
    }
    return {
        "constants": {"a_base": fwpatch.A_BASE, "cut": fwpatch.CUT, "b_base": fwpatch.B_BASE,
                      "cave": [fwpatch.CAVE_START, fwpatch.CAVE_END], "dead_code": list(fwpatch.DEAD_CODE),
                      "branch_sites": list(fwpatch.BRANCH_SITES), "known_bases": fwpatch.KNOWN_BASES,
                      "size": SIZE},
        "sites": [[s.va, s.original.hex(), s.patched.hex(), s.why] for s in fwpatch.RECORD_READ],
        "extras": {name: {str(va): data.hex() for va, data in extra.items()} for name, extra in extras.items()},
        "cases": cases,
        "refs": refs,
    }


# ---------------------------------------------------------------------- hardware


def hardware_section() -> dict:
    models = ["DRS-8500", " DRS-8500 ", "DRS-8500\t", "XYZ-1", "", "DR-8500UV", "drs-8500", "\x1cDRS-8500　"]
    return {
        "reported_model": hardware.REPORTED_MODEL, "model_number": hardware.MODEL_NUMBER,
        "platform": hardware.PLATFORM,
        "radios": [[r.brand, r.model, r.note, r.name] for r in hardware.SUPPORTED_RADIOS],
        "supported_names": hardware.supported_names(), "supported_text": hardware.supported_text(),
        "known_as": [[m, hardware.known_as(m)] for m in models],
    }


def main() -> None:
    # the messages hold characters outside the Windows console code page (cp950)
    sys.stdout.reconfigure(encoding="utf-8")
    json.dump({"iap": {"images": iap_images(), "scenarios": iap_scenarios(), "retry": iap.RETRY},
               "recording": recording_section(), "firmware_check": firmware_check_section(),
               "fwpatch": fwpatch_section(), "hardware": hardware_section()},
              sys.stdout, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
