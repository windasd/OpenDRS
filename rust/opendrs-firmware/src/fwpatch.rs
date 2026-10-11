//! Firmware patches the CPS can apply to a vendor DR-8500UV image (port of `fwpatch.py`).
//!
//! RECORD_READ wires the firmware's own recording-download handlers to the PC receive loop
//! (they exist but are unreachable in the vendor builds):
//!
//! ```text
//! 0x03026E5C  bt16 0x03026E3C -> bt16 CAVE_R     ("not 'p'" used to drop the byte)
//! 0x03026EC6  bt16 0x03026E88 -> bt16 CAVE_H     ("not 0x88" used to drop the frame)
//! CAVE_R 0x03027024: cmpnei r3,'R' ; bf 0x03026F04 (the 'F' command path) ; br 0x03026E3C
//! CAVE_H 0x0302702C: cmpnei r3,0x58 ; bt 0x03026E88 ; mov r0,r4 ; bsr 0x030272C4 (list-frame
//!                    handler) ; bsr 0x03026DC0 ; br 0x03026E3C   -- mirrors the 0x88 path
//! ```
//!
//! The caves overwrite the start of a dead dispatcher at 0x03027024. The same 30 bytes apply to
//! both vendor builds seen so far (…_03_136174_400480 and …_05_144146_430440). Verified on a
//! DR-8500UV (BK4819) on 2026-10-09.
//!
//! Safety: every patched byte must hold its known original value; for an image that is not one
//! of the known vendor builds the whole file is also scanned (raw C-SKY V2 encodings, both
//! units) to make sure nothing else branches, calls, loads or points into the overwritten dead
//! code.

use std::fmt;

use crate::pytext::sha256_hex;

/// Unit A (application) is loaded at A_BASE, the file's first CUT bytes; unit B follows.
pub const A_BASE: u32 = 0x0300_C000;
pub const CUT: usize = 0x000A_9F5C;
pub const B_BASE: u32 = 0x0001_0000;
pub const CAVE_START: u32 = 0x0302_7024;
pub const CAVE_END: u32 = 0x0302_703E;
/// Instructions of the dead dispatcher (its pool follows).
pub const DEAD_CODE: (u32, u32) = (0x0302_7024, 0x0302_705E);
pub const BRANCH_SITES: [u32; 2] = [0x0302_6E5C, 0x0302_6EC6];

/// Vendor builds the patch was checked against (whole-file scan done offline).
pub const KNOWN_BASES: &[(&str, &str)] = &[
    (
        "fd52d62d980c59039da869c05f2c5802fb60a9fc639f4cd6d73680c3b896cc36",
        "C7000_DR8500UV_4819_03_136174_400480.bin",
    ),
    (
        "2267ee7a650490368bbf0de4197b542353864ef1ab82ea8cc54d9f20f0e526ab",
        "C7000_DR8500UV_4819_05_144146_430440.bin",
    ),
];

pub fn known_base(sha256: &str) -> Option<&'static str> {
    KNOWN_BASES
        .iter()
        .find(|(sha, _)| *sha == sha256)
        .map(|&(_, name)| name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    pub va: u32,
    pub original: &'static [u8],
    pub patched: &'static [u8],
    pub why: &'static str,
}

/// 32-bit instructions are stored high halfword first, each halfword little-endian.
pub const RECORD_READ: &[Site] = &[
    Site {
        va: 0x0302_6E5C,
        original: &[0xF0, 0x0B],
        patched: &[0xE4, 0x08],
        why: "bt16 -> CAVE_R（原本：不是 'p' 就丟棄）",
    },
    Site {
        va: 0x0302_6EC6,
        original: &[0xE1, 0x0B],
        patched: &[0xB3, 0x08],
        why: "bt16 -> CAVE_H（原本：不是 0x88 就丟棄）",
    },
    Site {
        va: 0x0302_7024,
        original: &[0xD2, 0x14, 0x26, 0x14],
        patched: &[0x43, 0xEB, 0x52, 0x00],
        why: "cmpnei r3,'R'",
    },
    Site {
        va: 0x0302_7028,
        original: &[0xC3, 0x6C],
        patched: &[0x6E, 0x0F],
        why: "bf 0x03026F04（交給 'F' 指令路徑）",
    },
    Site {
        va: 0x0302_702A,
        original: &[0x47, 0x6D],
        patched: &[0x09, 0x07],
        why: "br 0x03026E3C（其他照舊丟棄）",
    },
    Site {
        va: 0x0302_702C,
        original: &[0x3B, 0x6C, 0x4F, 0x6C],
        patched: &[0x43, 0xEB, 0x58, 0x00],
        why: "cmpnei r3,0x58",
    },
    Site {
        va: 0x0302_7030,
        original: &[0x97, 0x6C],
        patched: &[0x2C, 0x0B],
        why: "bt 0x03026E88（不是 0x58 照舊丟棄）",
    },
    Site {
        va: 0x0302_7032,
        original: &[0x00, 0x33],
        patched: &[0x13, 0x6C],
        why: "mov r0,r4（封包緩衝區）",
    },
    Site {
        va: 0x0302_7034,
        original: &[0x60, 0xB8, 0x61, 0xB8],
        patched: &[0x00, 0xE0, 0x48, 0x01],
        why: "bsr 0x030272C4（清單封包處理）",
    },
    Site {
        va: 0x0302_7038,
        original: &[0x62, 0xB8, 0x63, 0xB8],
        patched: &[0xFF, 0xE3, 0xC4, 0xFE],
        why: "bsr 0x03026DC0",
    },
    Site {
        va: 0x0302_703C,
        original: &[0x64, 0xB8],
        patched: &[0x00, 0x07],
        why: "br 0x03026E3C",
    },
];

/// The record-read patch cannot be applied (Python: `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PatchError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchResult {
    pub data: Vec<u8>,
    pub base_sha256: String,
    pub sha256: String,
    pub changed: usize,
    /// Known vendor file name, or "" for an image checked by scanning.
    pub base_name: String,
}

/// File offset of a virtual address.
fn file_offset(va: u32) -> usize {
    let va = va as usize;
    let (a, b) = (A_BASE as usize, B_BASE as usize);
    if (a..a + CUT).contains(&va) {
        va - a
    } else {
        va.wrapping_sub(b).wrapping_add(CUT)
    }
}

/// Virtual address of a file offset.
fn virtual_address(offset: usize) -> i64 {
    // offsets come from slices, so they fit an i64
    let o = i64::try_from(offset).unwrap_or(i64::MAX);
    let cut = CUT as i64;
    if o < cut {
        i64::from(A_BASE) + o
    } else {
        i64::from(B_BASE) + (o - cut)
    }
}

fn sign_extend(value: u32, bits: u32) -> i64 {
    let v = i64::from(value);
    if v & (1 << (bits - 1)) != 0 {
        v - (1 << bits)
    } else {
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// bsr32 / br32 / bt32 / bf32 / lrw32 / jmpi32 / jsri32.
    Wide,
    /// br16 / bt16 / bf16.
    Branch16,
    /// lrw16.
    Load16,
    /// A 32-bit value pointing into the range (either halfword order).
    Pointer,
}

/// One reference found by [`refs_into`]; displayed like the Python version's strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    pub at: i64,
    /// The target address (`None` for a pointer).
    pub target: Option<i64>,
    pub kind: RefKind,
}

/// `f"{value:08X}"` (a negative value keeps its sign, as in Python).
fn hex8(value: i64) -> String {
    if value < 0 {
        format!("-{:07X}", value.unsigned_abs())
    } else {
        format!("{value:08X}")
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = hex8(self.at);
        match (self.kind, self.target.map(hex8)) {
            (RefKind::Pointer, _) | (_, None) => write!(f, "{at}:ptr"),
            (RefKind::Wide, Some(t)) => write!(f, "{at}->{t}"),
            (RefKind::Branch16, Some(t)) => write!(f, "{at}->{t}(b16)"),
            (RefKind::Load16, Some(t)) => write!(f, "{at}->{t}(lrw16)"),
        }
    }
}

/// The target of a 32-bit instruction starting with halfwords `h`, `n` at `at`.
fn wide_target(at: i64, h: u32, n: u32) -> Option<i64> {
    let w = (h << 16) | n;
    if w >> 26 == 0b11_1000 {
        return Some(at + sign_extend(w & 0x3FF_FFFF, 26) * 2); // bsr32
    }
    if h >> 8 == 0xE8 && h & 0x1F == 0 {
        return Some(at + sign_extend(n, 16) * 2); // br32 / bt32 / bf32
    }
    // 32-bit "compare register with zero and branch" (0xE800 | sop << 5 | rx, then a 16-bit
    // offset): 1 bnezad, 8 bez, 9 bnez, 10 bhz, 11 blsz, 12 blz, 13 bhsz.
    // e.g. bez r13 = E90D at 0x03018928.
    if h >> 10 == 0b11_1010 && matches!((h >> 5) & 0x1F, 1 | 8..=13) {
        return Some(at + sign_extend(n, 16) * 2);
    }
    if h >> 10 == 0b11_1010 && matches!((h >> 5) & 0x1F, 0x14 | 0x16 | 0x17) {
        return Some((at & !3) + i64::from(n) * 4); // lrw32 / jmpi32 / jsri32
    }
    None
}

/// Every reference a halfword pair at `at` makes into `[lo, hi)`.
fn refs_at(at: i64, h: u32, n: u32, lo: i64, hi: i64, out: &mut Vec<Reference>) {
    let inside = |t: i64| lo <= t && t < hi;
    let mut add = |target: Option<i64>, kind: RefKind| out.push(Reference { at, target, kind });
    if let Some(t) = wide_target(at, h, n).filter(|&t| inside(t)) {
        add(Some(t), RefKind::Wide);
    }
    if matches!(h >> 10, 1..=3) {
        let t = at + sign_extend(h & 0x3FF, 10) * 2; // br16 / bt16 / bf16
        if inside(t) {
            add(Some(t), RefKind::Branch16);
        }
    }
    if h >> 10 == 0b00_0100 {
        // lrw16 (0x14xx = push/pop)
        let t = (at & !3) + i64::from((((h >> 8) & 3) << 5) | (h & 0x1F)) * 4;
        if inside(t) {
            add(Some(t), RefKind::Load16);
        }
    }
    if inside(i64::from((h << 16) | n)) || inside(i64::from((n << 16) | h)) {
        add(None, RefKind::Pointer);
    }
}

/// Raw scan of both units for branches, calls, PC-relative loads and pointers into [lo, hi).
pub fn refs_into(data: &[u8], lo: i64, hi: i64) -> Vec<Reference> {
    let halves: Vec<u32> = data
        .chunks_exact(2)
        .map(|p| u32::from(u16::from_le_bytes([p[0], p[1]])))
        .collect();
    let mut out = Vec::new();
    for (i, pair) in halves.windows(2).enumerate() {
        refs_at(virtual_address(i * 2), pair[0], pair[1], lo, hi, &mut out);
    }
    out
}

fn site_bytes<'d>(data: &'d [u8], site: &Site) -> Option<&'d [u8]> {
    let o = file_offset(site.va);
    data.get(o..o.checked_add(site.original.len())?)
}

pub fn is_record_read_patched(data: &[u8]) -> bool {
    data.len() > CUT
        && RECORD_READ
            .iter()
            .all(|s| site_bytes(data, s) == Some(s.patched))
}

/// Whether the record-read patch fits an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordReadStatus {
    Patched,
    Patchable,
    /// Why the patch does not fit (for the UI).
    Unfit(String),
}

impl fmt::Display for RecordReadStatus {
    /// "patched", "patchable" or the reason, exactly as the Python version returns them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordReadStatus::Patched => f.write_str("patched"),
            RecordReadStatus::Patchable => f.write_str("patchable"),
            RecordReadStatus::Unfit(reason) => f.write_str(reason),
        }
    }
}

pub fn record_read_status(data: &[u8]) -> RecordReadStatus {
    if is_record_read_patched(data) {
        return RecordReadStatus::Patched;
    }
    if data.len() <= CUT {
        return RecordReadStatus::Unfit(
            "檔案太小，不是 DR-8500UV／DH-98VU（C7000 平台）的完整韌體".into(),
        );
    }
    let wrong = RECORD_READ
        .iter()
        .filter(|s| site_bytes(data, s) != Some(s.original))
        .count();
    if wrong > 0 {
        return RecordReadStatus::Unfit(format!(
            "程式碼與已知的原廠版本不同（{wrong} 個修補點不符），無法套用"
        ));
    }
    RecordReadStatus::Patchable
}

/// References into `[lo, hi)` from code outside it, as text.
fn refs_from_outside(data: &[u8], lo: u32, hi: u32) -> Vec<String> {
    let (lo, hi) = (i64::from(lo), i64::from(hi));
    refs_into(data, lo, hi)
        .iter()
        .filter(|r| !(lo <= r.at && r.at < hi))
        .map(Reference::to_string)
        .collect()
}

fn first_five(refs: &[String]) -> String {
    refs.iter()
        .take(5)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("、")
}

pub fn patch_record_read(data: &[u8]) -> Result<PatchResult, PatchError> {
    match record_read_status(data) {
        RecordReadStatus::Patched => {
            return Err(PatchError("這個檔案已經是錄音讀取版".into()));
        }
        RecordReadStatus::Unfit(reason) => return Err(PatchError(reason)),
        RecordReadStatus::Patchable => {}
    }
    let base = sha256_hex(data);
    let base_name = known_base(&base).unwrap_or_default();
    if base_name.is_empty() {
        // unknown build with matching bytes: prove the overwritten code is dead
        let outside = refs_from_outside(data, DEAD_CODE.0, DEAD_CODE.1);
        if !outside.is_empty() {
            return Err(PatchError(format!(
                "檢查到有程式會進入被覆寫的區域，不套用：{}",
                first_five(&outside)
            )));
        }
    }
    let mut out = data.to_vec();
    for site in RECORD_READ {
        let o = file_offset(site.va);
        out[o..o + site.patched.len()].copy_from_slice(site.patched);
    }
    if base_name.is_empty() {
        let mut into = refs_from_outside(&out, CAVE_START, CAVE_END);
        into.sort();
        if into != ["03026E5C->03027024(b16)", "03026EC6->0302702C(b16)"] {
            return Err(PatchError(format!(
                "修補後的進入點不符預期，不套用：{}",
                first_five(&into)
            )));
        }
    }
    let changed = data.iter().zip(&out).filter(|(a, b)| a != b).count();
    Ok(PatchResult {
        sha256: sha256_hex(&out),
        data: out,
        base_sha256: base,
        changed,
        base_name: base_name.to_owned(),
    })
}
