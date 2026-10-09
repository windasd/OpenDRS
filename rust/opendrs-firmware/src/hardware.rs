//! Radios this program supports: one hardware platform (Kydera "C7000") sold under several
//! names (port of `hardware.py`).
//!
//! The firmware identifies every unit the same way — the PC read reports model `DRS-8500` and
//! model number `DR8600` (codeplug image +0x70 / +0x20) and the firmware ID
//! `308E.<chip>TARSCB.003` — so the brand on the front panel cannot be told from the radio's
//! data. HORA DH-98VU is a re-badged Kydera DR-8500UV (same vendor firmware files; the reference
//! project `tests/fixtures/dh98vu_20250811.F136` holds exactly these identifiers).

use opendrs_core::pystr::strip;

/// What the radio reports when read by the PC.
pub const REPORTED_MODEL: &str = "DRS-8500";
/// Codeplug image +0x20.
pub const MODEL_NUMBER: &str = "DR8600";
pub const PLATFORM: &str = "Kydera C7000 平台";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportedRadio {
    pub brand: &'static str,
    pub model: &'static str,
    pub note: &'static str,
}

impl SupportedRadio {
    pub fn name(&self) -> String {
        format!("{} {}", self.brand, self.model)
    }
}

pub const SUPPORTED_RADIOS: &[SupportedRadio] = &[
    SupportedRadio {
        brand: "Kydera",
        model: "DR-8500UV",
        note: "原廠機型",
    },
    SupportedRadio {
        brand: "Kydera",
        model: "DRS-8500",
        note: "對講機寫頻時回報的機型名稱",
    },
    SupportedRadio {
        brand: "HORA",
        model: "DH-98VU",
        note: "Kydera DR-8500UV 的貼牌機；讀寫頻、IAP 升級與錄音讀取皆已實機驗證",
    },
];

fn names<'a>(radios: impl Iterator<Item = &'a SupportedRadio>) -> String {
    radios
        .map(SupportedRadio::name)
        .collect::<Vec<_>>()
        .join("／")
}

/// "Kydera DR-8500UV／Kydera DRS-8500／HORA DH-98VU".
pub fn supported_names() -> String {
    names(SUPPORTED_RADIOS.iter())
}

pub fn supported_text() -> String {
    format!(
        "{}（同一硬體平台，寫頻時回報機型 {REPORTED_MODEL}、型號 {MODEL_NUMBER}）",
        supported_names()
    )
}

/// Other names for the model a radio or project reports ("" when it is not one of ours).
pub fn known_as(model: &str) -> String {
    if strip(model) != REPORTED_MODEL {
        return String::new();
    }
    names(
        SUPPORTED_RADIOS
            .iter()
            .filter(|r| r.model != REPORTED_MODEL),
    )
}
