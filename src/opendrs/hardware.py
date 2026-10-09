"""Radios this program supports: one hardware platform (Kydera "C7000") sold under several names.

The firmware itself identifies every unit the same way — the PC read reports model
``DRS-8500`` and model number ``DR8600`` (codeplug image +0x70 / +0x20) and the firmware ID
``308E.<chip>TARSCB.003`` — so the brand on the front panel cannot be told from the radio's
data. HORA DH-98VU is a re-badged Kydera DR-8500UV: its vendor download bundle ships the same
firmware files (C7000_DR8500UV_4819_03/05) and the reference project
``tests/fixtures/dh98vu_20250811.F136`` (a DH-98VU) holds exactly these identifiers.
"""

from __future__ import annotations

from dataclasses import dataclass

REPORTED_MODEL = "DRS-8500"   # what the radio reports when read by the PC
MODEL_NUMBER = "DR8600"       # codeplug image +0x20
PLATFORM = "Kydera C7000 平台"


@dataclass(frozen=True)
class SupportedRadio:
    brand: str
    model: str
    note: str

    @property
    def name(self) -> str:
        return f"{self.brand} {self.model}"


SUPPORTED_RADIOS: tuple[SupportedRadio, ...] = (
    SupportedRadio("Kydera", "DR-8500UV", "原廠機型"),
    SupportedRadio("Kydera", "DRS-8500", "對講機寫頻時回報的機型名稱"),
    SupportedRadio("HORA", "DH-98VU", "Kydera DR-8500UV 的貼牌機；讀寫頻、IAP 升級與錄音讀取皆已實機驗證"),
)


def supported_names() -> str:
    """"Kydera DR-8500UV／Kydera DRS-8500／HORA DH-98VU"."""
    return "／".join(r.name for r in SUPPORTED_RADIOS)


def supported_text() -> str:
    return (f"{supported_names()}（同一硬體平台，寫頻時回報機型 {REPORTED_MODEL}、型號 {MODEL_NUMBER}）")


def known_as(model: str) -> str:
    """Other names for the model a radio or project reports ("" when it is not one of ours)."""
    if model.strip() != REPORTED_MODEL:
        return ""
    return "／".join(r.name for r in SUPPORTED_RADIOS if r.model != REPORTED_MODEL)
