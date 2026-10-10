"""Export what the Python implementation computes for every fixture, as the Rust port's oracle.

Run from the repository root with the Python package importable:
    PYTHONPATH=../opendrs-oracle/src python rust/tools/make_parity.py > rust/opendrs-core/tests/parity.json
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

from opendrs import codeplug, f136, textcodec
from opendrs.codeplug import Counts, build_stream, check_writable, counts_in_image, parse_stream
from opendrs.radio_link import RadioIdentity
from opendrs.records import hz_to_mhz, mhz_to_hz
from opendrs.textcodec import NameCodec

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "rust" / "fixtures"
CODECS = (NameCodec.BIG5, NameCodec.GBK, NameCodec.ASCII)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def project_facts(project: f136.Project) -> dict:
    dev = project.device
    try:
        stream = build_stream(project)
        stream_sha = sha(stream)
    except codeplug.CodeplugError:
        stream_sha = None
    counts = counts_in_image(project.image)
    return {
        "contacts": [[c.index, c.kind, c.kind_label, *[c.name(k) for k in CODECS], c.call_id]
                     for c in project.contacts],
        "zones": [[z.index, z.name(NameCodec.BIG5), z.first_record, z.channel_count] for z in project.zones],
        "channels": [[c.index, c.name(NameCodec.BIG5), c.freq_a_hz, c.freq_b_hz, c.mode, c.mode_label,
                      c.color_code, c.rx_group, c.tx_contact, hz_to_mhz(c.freq_a_hz), hz_to_mhz(c.freq_b_hz)]
                     for c in project.channels],
        "device": {"band_label": dev.band_label, "write_date": dev.write_date, "model": dev.model,
                   "alias": dev.alias, "dmr_id": dev.dmr_id, "firmware": dev.firmware,
                   "firmware_date": dev.firmware_date, "factory_number": dev.factory_number,
                   "serial_number": dev.serial_number, "model_number": dev.model_number,
                   "bands": [list(b) for b in dev.bands] if dev.bands else None},
        "problems": project.problems(),
        "check_writable": check_writable(project),
        "names_decodable": {k.value: project.names_decodable(k) for k in CODECS},
        "suggest_codec": (lambda s: s.value if s else None)(textcodec.suggest_codec(
            [c.raw[3:13] for c in project.contacts] + [z.raw[3:13] for z in project.zones]
            + [c.raw[2:12] for c in project.channels])),
        "counts_in_image": [counts.zones, counts.channels, counts.contacts] if counts else None,
        "section_order": list(project.section_order),
        "image_len": len(project.image),
        "serialize_sha256": sha(f136.serialize(project)),
        "build_stream_sha256": stream_sha,
    }


def textcodec_cases() -> dict:
    # Big5 codes where the WHATWG tables differ from Python: A145 (bullet), A2A4–A2A7, A2CC, A2CE, the
    # ETEN kana / Cyrillic (C6A1…C7FC), box drawing F9F9–F9FE, a duplicate (C94A) and user-defined areas.
    raws = [b"QA0EL\r\n\x00\x00\x00", b"\xa4\xa4\xa4\xe5\x00", b"\xd6\xd0\xce\xc4\xff\xff", b"\x81\x30\x00",
            b"\xff\xff\xff", b"", b"A\x01B", b"\xa1\x40", b"\x80", b"\xa1\x45\xa2\xa4\xa2\xcc\xa2\xce",
            b"\xc6\xa1\xc6\xe7\xc7\xf3\xc7\xfc", b"\xf9\xf9\xf9\xfe\xc9\x4a", b"\xfa\x40", b"\x8e\x40", b"\xa1\xaa\xa1\xa4",
            b"\xa3\xe1", b"\xc8\x7e"]
    display = [[r.hex(), k.value, textcodec.display_name(r, k), textcodec.decode_name(r, k)]
               for r in raws for k in CODECS]
    texts = ["TG46600_TW", "中文", "测试", "頻道一二三四五", "José", "€", "a\x00b", "", "  sp  ", "12345678901",
             "ÅÄÖ", "台灣", "测", "Дмитрий", "═╡", "あいう", "兀十", "／＼", "•‧", "ㄅㄆ", "—―", "·・", "Ω"]
    encode = []
    for t in texts:
        for k in CODECS:
            try:
                encode.append([t, k.value, textcodec.encode_name(t, 10, k).hex(), None])
            except textcodec.NameEncodingError as exc:
                encode.append([t, k.value, None, str(exc)])
    fit = [[t, k.value, *(lambda r: [r[0].hex(), r[1]])(textcodec.fit_name(t, 10, k))]
           for t in ["José Núñez", "Ångström Ltd", "王小明工作室名稱很長", "a\tb\nc", "QA0BD James", "ŞĞÜ",
                     "Дмитрий Иванов", "ﾊﾝｶｸ", "═ BOX ═"]
           for k in CODECS]
    blobs = ["頻道設定".encode("big5"), "频道设定".encode("gbk"), "測試".encode("big5") + b" abc", b"plain ascii",
             "聯絡人".encode("gbk"), b"\xa4\xa4\xa4\xe5\xa2\xa4", "日本語かな".encode("big5")]
    cjk = [[b.hex(), *(textcodec.best_cjk_decoding(b) or [None, None])] for b in blobs]
    legacy = [b"\xef\xbb\xbfhello", "中文".encode("utf-16"), "設定檔".encode("big5"), b"caf\xe9", b"plain",
              b"\xfe\xff" + "中文".encode("utf-16-be"), b"\xff\xfe\x00\x00odd", b"\xfe\xff\xd8\x00",
              b"\xff\xfe\x00\xdc\x41\x00", b"\xa4\xa4\xa4\xe5\xa2\xa4", "Дмитрий ═".encode("big5")]
    legacy_cases = [[b.hex(), *textcodec.read_legacy_text(b)] for b in legacy]
    return {"display": display, "encode": encode, "fit": fit, "cjk": cjk, "legacy": legacy_cases}


def mhz_cases() -> dict:
    inputs = ["438.8", " 145.5 ", "439.85000", "1e2", "430.0000005", "abc", "", "-1", "438.80000001",
              "4.3885E2", "+144.8", "４３８.８", "1_44.5", "　145\x1c", "-0", "0E999999999", "inf", "NaN",
              "438.80000000000000000000000001", "1E999999999", "1E999999999999999999", "9223372036854.775808",
              "-9223372036854.775808", "9223372036854.775807", "1 44", "1e", ".5e-6", "5E-7",
              "0E999999999999999999", "0E999999999999999993", "-0E-5"]
    out = []
    for text in inputs:
        try:
            out.append([text, mhz_to_hz(text), None])
        except ValueError as exc:
            out.append([text, None, str(exc)])
    hz = [0, 1, 439_850_000, 144_040_000, 467_512_500, 433_000_001, 4_294_967_295]
    return {"mhz_to_hz": out, "hz_to_mhz": [[h, hz_to_mhz(h)] for h in hz]}


def main() -> None:
    fixtures = {}
    for path in [FIX / "dh98vu_20250811.F136", FIX / "mmdvm_20241124.F136", FIX / "mmdvm_20241127.F136",
                 FIX / "capture" / "written_project.F136"]:
        project = f136.load(path)
        facts = project_facts(project)
        facts["file_sha256"] = sha(path.read_bytes())
        fixtures[str(path.relative_to(FIX)).replace("\\", "/")] = facts
    for name, counts in (("capture/read_stream.bin", Counts(4, 27, 360)),
                         ("capture/read1_stream.bin", Counts(4, 26, 360))):
        project = parse_stream((FIX / name).read_bytes(), counts)
        fixtures[name] = project_facts(project)
    ident = RadioIdentity.parse((FIX / "capture" / "read_reply.bin").read_bytes()[22:])
    json.dump({"fixtures": fixtures, "textcodec": textcodec_cases(), "mhz": mhz_cases(),
               "identity": {"firmware": ident.firmware, "model": ident.model, "band": ident.band,
                            "summary": ident.summary()}},
              sys.stdout, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
