//! General radio settings stored as bit fields in the settings image (port of `radio_settings.py`).
//!
//! Each field is declared once (page, label, offset, bit mask, and either a list of choices or a
//! numeric range). Values are read and written through the mask only, so neighbouring settings
//! that share a byte are never disturbed. The offsets and encodings were mapped by driving the
//! legacy CPS, so each choice is the exact raw value it writes.

use std::sync::LazyLock;

/// A value a setting cannot take, or an image too short to hold it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingError(pub String);

/// `(label, raw value within the mask)`.
pub type Choice = (String, u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    pub key: String,
    pub page: &'static str,
    pub label: String,
    pub offset: usize,
    pub mask: u32,
    pub choices: Vec<Choice>,
    pub minimum: i64,
    pub maximum: i64,
    /// Shown value = raw × scale (e.g. a time stored in tens of seconds).
    pub scale: i64,
    pub unit: &'static str,
    /// Bytes, little-endian, for multi-byte numbers.
    pub size: usize,
    pub note: &'static str,
    pub depends: Vec<&'static str>,
}

impl Setting {
    pub fn new(
        key: impl Into<String>,
        page: &'static str,
        label: impl Into<String>,
        offset: usize,
        mask: u32,
    ) -> Self {
        Setting {
            key: key.into(),
            page,
            label: label.into(),
            offset,
            mask,
            choices: Vec::new(),
            minimum: 0,
            maximum: 0,
            scale: 1,
            unit: "",
            size: 1,
            note: "",
            depends: Vec::new(),
        }
    }

    pub fn with_choices(self, choices: Vec<Choice>) -> Self {
        Setting { choices, ..self }
    }

    pub fn with_range(self, minimum: i64, maximum: i64) -> Self {
        Setting {
            minimum,
            maximum,
            ..self
        }
    }

    pub fn depending_on(self, keys: &[&'static str]) -> Self {
        Setting {
            depends: keys.to_vec(),
            ..self
        }
    }

    pub fn noted(self, note: &'static str) -> Self {
        Setting { note, ..self }
    }

    pub fn shift(&self) -> u32 {
        self.mask.trailing_zeros()
    }

    pub fn is_choice(&self) -> bool {
        !self.choices.is_empty()
    }

    fn word(&self, image: &[u8]) -> Option<u32> {
        let bytes = image.get(self.offset..self.offset + self.size)?;
        Some(
            bytes
                .iter()
                .rev()
                .fold(0u32, |acc, b| (acc << 8) | u32::from(*b)),
        )
    }

    pub fn raw(&self, image: &[u8]) -> Option<u32> {
        self.word(image)
            .map(|value| (value & self.mask) >> self.shift())
    }

    pub fn display(&self, image: &[u8]) -> String {
        let Some(raw) = self.raw(image) else {
            return "—".into();
        };
        if self.is_choice() {
            return self
                .choices
                .iter()
                .find(|(_, v)| *v == raw)
                .map_or_else(|| format!("未知值 {raw}"), |(label, _)| label.clone());
        }
        format!("{}{}", i64::from(raw) * self.scale, self.unit)
    }

    pub fn with_raw(&self, image: &[u8], raw: i64) -> Result<Vec<u8>, SettingError> {
        let old = self
            .word(image)
            .ok_or_else(|| SettingError("設定區太短，無法寫入此設定".into()))?;
        let limit = i64::from(self.mask >> self.shift());
        if !(0..=limit).contains(&raw) {
            return Err(SettingError(format!("{}：值超出範圍", self.label)));
        }
        let raw =
            u32::try_from(raw).map_err(|_| SettingError(format!("{}：值超出範圍", self.label)))?;
        if self.is_choice() && !self.choices.iter().any(|(_, v)| *v == raw) {
            return Err(SettingError(format!("{}：不是有效的選項", self.label)));
        }
        if !self.is_choice()
            && !(self.minimum..=self.maximum).contains(&(i64::from(raw) * self.scale))
        {
            return Err(SettingError(format!(
                "{}：必須介於 {} 與 {}{}",
                self.label, self.minimum, self.maximum, self.unit
            )));
        }
        let new = (old & !self.mask) | (raw << self.shift());
        let mut out = image.to_vec();
        out[self.offset..self.offset + self.size].copy_from_slice(&new.to_le_bytes()[..self.size]);
        Ok(out)
    }

    /// Numeric settings: `value` in display units (must be a multiple of the scale).
    pub fn with_value(&self, image: &[u8], value: i64) -> Result<Vec<u8>, SettingError> {
        if value % self.scale != 0 {
            return Err(SettingError(format!(
                "{}：必須是 {} 的倍數",
                self.label, self.scale
            )));
        }
        self.with_raw(image, value / self.scale)
    }
}

/// A NUL-padded ASCII text field (e.g. the DTMF code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSetting {
    pub key: String,
    pub page: &'static str,
    pub label: String,
    pub offset: usize,
    pub size: usize,
    /// Permitted characters ("" = any printable ASCII).
    pub allowed: &'static str,
    pub note: &'static str,
}

impl TextSetting {
    pub fn text(&self, image: &[u8]) -> Option<String> {
        let chunk = image.get(self.offset..self.offset + self.size)?;
        let end = chunk.iter().position(|b| *b == 0).unwrap_or(chunk.len());
        Some(
            chunk[..end]
                .iter()
                .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
                .collect(),
        )
    }

    pub fn with_text(&self, image: &[u8], text: &str) -> Result<Vec<u8>, SettingError> {
        if image.len() < self.offset + self.size {
            return Err(SettingError("設定區太短，無法寫入此設定".into()));
        }
        let text = text.trim();
        if text.chars().count() > self.size {
            return Err(SettingError(format!(
                "{}：最多 {} 個字元",
                self.label, self.size
            )));
        }
        let bad = text.chars().find(|ch| {
            (!self.allowed.is_empty() && !self.allowed.contains(*ch)) || !(' '..'\x7f').contains(ch)
        });
        if let Some(bad) = bad {
            let hint = if self.allowed.is_empty() {
                String::new()
            } else {
                format!("（只能用 {}）", self.allowed)
            };
            return Err(SettingError(format!(
                "{}：不可使用「{bad}」{hint}",
                self.label
            )));
        }
        let mut out = image.to_vec();
        let field = &mut out[self.offset..self.offset + self.size];
        field.fill(0);
        field[..text.len()].copy_from_slice(text.as_bytes());
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadioSetting {
    Bits(Setting),
    Text(TextSetting),
}

impl RadioSetting {
    pub fn key(&self) -> &str {
        match self {
            RadioSetting::Bits(s) => &s.key,
            RadioSetting::Text(s) => &s.key,
        }
    }
    pub fn page(&self) -> &'static str {
        match self {
            RadioSetting::Bits(s) => s.page,
            RadioSetting::Text(s) => s.page,
        }
    }
    pub fn label(&self) -> &str {
        match self {
            RadioSetting::Bits(s) => &s.label,
            RadioSetting::Text(s) => &s.label,
        }
    }
}

/// Consecutive raw values from `start`.
pub fn choices(labels: &[&str], start: u32) -> Vec<Choice> {
    labels
        .iter()
        .zip(start..)
        .map(|(l, v)| (l.to_string(), v))
        .collect()
}

/// One choice per value: label and raw value derived from it.
pub fn labelled(
    values: impl IntoIterator<Item = i64>,
    label: impl Fn(i64) -> String,
    raw: impl Fn(i64) -> i64,
) -> Vec<Choice> {
    values
        .into_iter()
        .map(|v| (label(v), u32::try_from(raw(v)).unwrap_or(0)))
        .collect()
}

fn same(v: i64) -> i64 {
    v
}

fn off_and(rest: Vec<Choice>) -> Vec<Choice> {
    std::iter::once(("Off".to_string(), 0))
        .chain(rest)
        .collect()
}

pub fn off_on() -> Vec<Choice> {
    choices(&["Off", "On"], 0)
}
fn hide_show() -> Vec<Choice> {
    choices(&["隱藏", "顯示"], 0)
}

pub const BASIC: &str = "基本設置";
pub const MENU: &str = "菜單選項";
pub const TONE: &str = "提示音";
pub const LED: &str = "指示燈";
pub const KEYS: &str = "自定義按鍵";
pub const MIC: &str = "MIC增益";
pub const DMR: &str = "DMR服務項目";
pub const ENC: &str = "加密";

fn key_functions() -> Vec<Choice> {
    let mut out = choices(
        &[
            "未定義",
            "高低功率切換",
            "背景燈開/關",
            "鍵盤鎖",
            "聲控開關",
            "區域切換",
            "掃描開/關",
            "掃描模式",
            "中繼/脫網",
            "緊急開關",
            "加密開/關",
            "聯系人",
            "短信",
            "對講機激活",
            "對講機檢測",
            "對講機遙斃",
            "遠程監聽",
            "監聽",
            "永久監聽",
        ],
        0,
    );
    out.extend(
        [
            ("DTMF開關", 49),
            ("漫游開關", 52),
            ("中轉開關", 53),
            ("中轉監聽開關", 54),
            ("單雙守候開關", 63),
        ]
        .map(|(l, v)| (l.to_string(), v)),
    );
    out
}

fn tone_pick() -> Vec<Choice> {
    off_and(labelled(1..6, |v| v.to_string(), same))
}

/// `(group, byte, [(bit, entry)])`.
pub type MenuGroup = (&'static str, usize, &'static [(u32, &'static str)]);

/// The menu switches, read by the firmware as one 0x12-byte block from 0x3C00AE.
pub const MENU_GROUPS: &[MenuGroup] = &[
    (
        "聯系人",
        0xAE,
        &[(0, "聯系人列表"), (1, "新建聯系人"), (2, "手動撥號")],
    ),
    (
        "增強業務",
        0xBA,
        &[
            (0, "檢測"),
            (1, "呼叫提示"),
            (2, "遠程監聽"),
            (3, "遙斃"),
            (4, "激活"),
        ],
    ),
    (
        "短信",
        0xAF,
        &[
            (0, "寫信息"),
            (1, "常用短信"),
            (2, "收信箱"),
            (3, "發信箱"),
            (4, "草稿箱"),
        ],
    ),
    (
        "通話記錄",
        0xB0,
        &[(0, "呼出號碼"), (1, "已接來電"), (2, "未接來電")],
    ),
    (
        "掃描",
        0xB1,
        &[
            (0, "掃描開關"),
            (1, "掃描列表"),
            (2, "掃描模式"),
            (3, "漫遊開關"),
        ],
    ),
    ("區域設置", 0xB2, &[(0, "區域列表")]),
    (
        "對講機設置",
        0xB3,
        &[
            (0, "語言"),
            (1, "鎖鍵"),
            (2, "LCD背景燈"),
            (3, "LED指示燈"),
            (4, "顯示模式"),
            (5, "聲控"),
            (6, "信道開關"),
            (7, "恢復出廠設置"),
        ],
    ),
    (
        "參數設置",
        0xB5,
        &[
            (0, "呼叫時限"),
            (1, "功率設置"),
            (2, "中繼設置"),
            (3, "省電模式"),
            (4, "靜噪級別"),
            (5, "寬窄帶"),
            (6, "繁忙開關"),
            (7, "信令設置"),
        ],
    ),
    ("加密設置", 0xB4, &[(0, "加密設置")]),
    (
        "提示音設置",
        0xB6,
        &[
            (0, "情景"),
            (1, "按鍵"),
            (2, "電量"),
            (3, "短信"),
            (4, "個呼結束"),
            (5, "組呼結束"),
            (6, "呼叫提示"),
            (7, "開機提示音"),
        ],
    ),
    (
        "附件",
        0xB7,
        &[(0, "GPS"), (3, "時間"), (4, "DTMF"), (5, "手咪喇叭")],
    ),
    (
        "錄音",
        0xB8,
        &[
            (0, "錄音設置"),
            (1, "錄音列表"),
            (2, "清空錄音"),
            (3, "查詢空間"),
        ],
    ),
    (
        "設備信息",
        0xB9,
        &[
            (0, "本機個號"),
            (1, "接收組列表"),
            (2, "默認聯系人"),
            (3, "版本號"),
        ],
    ),
    ("信道編輯", 0xBB, &[(0, "信道編輯")]),
];

fn bits(
    key: &str,
    page: &'static str,
    label: &str,
    offset: usize,
    mask: u32,
    choices: Vec<Choice>,
) -> Setting {
    Setting::new(key, page, label, offset, mask).with_choices(choices)
}

fn ms_steps(from: i64, to: i64, step: i64, divisor: i64) -> Vec<Choice> {
    labelled(
        (from..=to).step_by(step as usize),
        |v| format!("{v}ms"),
        move |v| v / divisor,
    )
}

fn build_settings() -> Vec<RadioSetting> {
    let mut s: Vec<Setting> = vec![
        // ---- 基本設置 (radio name and ID are on the device page)
        bits(
            "language",
            BASIC,
            "語言設置",
            0x95,
            0x10,
            choices(&["中文", "英文"], 0),
        ),
        bits(
            "tot",
            BASIC,
            "限時發射",
            0x134F,
            0xFF,
            off_and(labelled(
                (20..=500).step_by(10),
                |s| format!("{s}S"),
                |s| s / 10,
            )),
        ),
        bits("busy_lockout", BASIC, "繁忙鎖定開關", 0xA6, 0x80, off_on()),
        bits("vox", BASIC, "聲控發射開關", 0xA6, 0x40, off_on()),
        bits(
            "vox_level",
            BASIC,
            "聲控發送靈敏度",
            0xA6,
            0x0F,
            labelled(1..13, |v| v.to_string(), |v| v - 1),
        ),
        bits("power_save", BASIC, "省電模式開關", 0x96, 0x04, off_on()),
        bits(
            "power_save_ratio",
            BASIC,
            "省電模式",
            0x96,
            0x18,
            choices(&["1:1", "1:2", "1:4"], 0),
        )
        .depending_on(&["power_save"]),
        bits(
            "power_save_delay",
            BASIC,
            "省電啟動時間",
            0xA3,
            0xFF,
            labelled(10..61, |v| format!("{v}S"), same),
        )
        .depending_on(&["power_save"]),
        bits(
            "scan_mode",
            BASIC,
            "掃描模式",
            0x137D,
            0x03,
            choices(&["CO", "TO", "SE"], 0),
        ),
        bits(
            "tail_tone",
            BASIC,
            "尾音類型",
            0x1381,
            0x03,
            choices(&["55Hz", "120°", "180°", "240°"], 0),
        ),
        bits(
            "squelch_a",
            BASIC,
            "靜噪(A)",
            0x93,
            0x0F,
            labelled(0..10, |v| v.to_string(), same),
        ),
        bits(
            "squelch_b",
            BASIC,
            "靜噪(B)",
            0x93,
            0xF0,
            labelled(0..10, |v| v.to_string(), same),
        ),
        bits(
            "backlight",
            BASIC,
            "背景燈開關",
            0x95,
            0x28,
            vec![("Off".into(), 0), ("On".into(), 1), ("Auto".into(), 4)],
        ),
        bits("keylock", BASIC, "鎖鍵開關", 0x95, 0x04, off_on()),
        bits("roaming", BASIC, "漫遊開關", 0x137B, 0x01, off_on()),
        bits(
            "roaming_mode",
            BASIC,
            "漫遊模式",
            0x1375,
            0x03,
            choices(&["自動", "手動", "強信號優先"], 0),
        ),
        bits(
            "roaming_rssi",
            BASIC,
            "Rssi信號",
            0x1376,
            0x3F,
            labelled(90..126, |v| format!("-{v}dBm"), |v| v - 90),
        ),
        bits(
            "site_search_timer",
            BASIC,
            "站點搜索計時器",
            0x1377,
            0xFF,
            labelled(0..256, |v| format!("{v}S"), same),
        ),
        bits(
            "site_confirm_timer",
            BASIC,
            "站點確認計時器",
            0x1378,
            0xFF,
            labelled(0..256, |v| format!("{v}S"), same),
        ),
        bits(
            "connect_count",
            BASIC,
            "連接次數",
            0x1379,
            0x0F,
            labelled(1..11, |v| v.to_string(), |v| v - 1),
        ),
        bits(
            "record",
            BASIC,
            "錄音設置",
            0x1380,
            0x03,
            choices(&["Off", "TX", "RX", "TX/RX"], 0),
        ),
        // ---- 提示音
        bits(
            "tone_profile",
            TONE,
            "情景模式",
            0xA7,
            0x01,
            choices(&["標準", "靜音"], 0),
        ),
        bits("tone_sms", TONE, "短信提示音", 0xA8, 0x07, tone_pick()),
        bits("tone_private", TONE, "個呼提示音", 0xA9, 0x07, tone_pick()),
        bits("tone_group", TONE, "組呼提示音", 0xAA, 0x07, tone_pick()),
        bits("tone_key", TONE, "按鍵提示音開關", 0xAB, 0x80, off_on()),
        bits(
            "tone_key_volume",
            TONE,
            "按鍵音量",
            0xAB,
            0x0F,
            labelled(1..14, |v| v.to_string(), same),
        )
        .depending_on(&["tone_key"]),
        bits(
            "tone_low_battery",
            TONE,
            "低電壓提示音開關",
            0xAC,
            0x80,
            off_on(),
        ),
        bits(
            "tone_low_battery_volume",
            TONE,
            "低電壓音量",
            0xAC,
            0x0F,
            labelled(1..14, |v| v.to_string(), same),
        )
        .depending_on(&["tone_low_battery"]),
        bits(
            "tone_call_hang",
            TONE,
            "呼叫掛起",
            0x12D8,
            0x01,
            choices(&["靜音", "提示音"], 0),
        ),
        bits(
            "tone_power_on",
            TONE,
            "開機提示音開關",
            0x95,
            0x02,
            off_on(),
        ),
        bits(
            "tone_roam_restart",
            TONE,
            "漫遊重啟提示音",
            0x1382,
            0x07,
            tone_pick(),
        ),
        bits(
            "tone_site_lock",
            TONE,
            "站點鎖定提示音",
            0x1383,
            0x07,
            tone_pick(),
        ),
        // ---- 指示燈: one byte, one bit per LED
        bits("led_device", LED, "設備指示燈開關", 0xAD, 0x10, off_on()),
        bits("led_tx", LED, "發射指示燈開關", 0xAD, 0x08, off_on()),
        bits("led_rx", LED, "接收指示燈開關", 0xAD, 0x04, off_on()),
        bits("led_scan", LED, "掃描狀態指示燈開關", 0xAD, 0x02, off_on()),
        bits(
            "led_low_battery",
            LED,
            "電池低電量指示燈開關",
            0xAD,
            0x01,
            off_on(),
        ),
        // ---- 自定義按鍵: P1-P3 long/short at 0xC2-0xC7
        bits(
            "key_long_time",
            KEYS,
            "長按持續時間",
            0xC1,
            0xFF,
            labelled(0..10, |i| format!("{:.1}S", (i + 1) as f64 * 0.5), same),
        ),
    ];
    for p in 1..=3usize {
        for (kind, label, shift) in [("long", "長按", 0usize), ("short", "短按", 1)] {
            s.push(bits(
                &format!("key_p{p}_{kind}"),
                KEYS,
                &format!("P{p}鍵 {label}"),
                0xC2 + 2 * (p - 1) + shift,
                0xFF,
                key_functions(),
            ));
        }
    }
    s.extend([
        // ---- MIC增益
        bits(
            "mic_gain1_on",
            MIC,
            "內置麥克風增益開關1",
            0xA4,
            0x80,
            off_on(),
        ),
        bits(
            "mic_gain1",
            MIC,
            "內置麥克風增益1",
            0xA4,
            0x07,
            labelled((0..21).step_by(4), |v| format!("{v}dB"), |v| v / 4),
        )
        .depending_on(&["mic_gain1_on"]),
        bits(
            "mic_gain2_on",
            MIC,
            "內置麥克風增益開關2",
            0xA5,
            0x80,
            off_on(),
        ),
        bits(
            "mic_gain2",
            MIC,
            "內置麥克風增益2",
            0xA5,
            0x3F,
            labelled(0..44, |v| format!("{v}dB"), same),
        )
        .depending_on(&["mic_gain2_on"]),
        // ---- DMR服務項目
        bits(
            "remote_monitor_time",
            DMR,
            "遠程監聽時間",
            0xFAC,
            0x0F,
            labelled((10..=120).step_by(10), |s| format!("{s}S"), |s| s / 10 - 1),
        ),
        bits(
            "decode_remote_monitor",
            DMR,
            "遠程監聽解碼（本機）",
            0xFAD,
            0x80,
            off_on(),
        ),
        bits(
            "decode_kill",
            DMR,
            "遙斃解碼（本機）",
            0xFAD,
            0x40,
            off_on(),
        ),
        bits(
            "decode_check",
            DMR,
            "檢測解碼（本機）",
            0xFAD,
            0x20,
            off_on(),
        ),
        bits(
            "decode_revive",
            DMR,
            "激活解碼（本機）",
            0xFAD,
            0x10,
            off_on(),
        ),
        bits("call_alert", DMR, "呼叫提示", 0xFAD, 0x08, off_on()),
        bits(
            "group_hang_time",
            DMR,
            "組呼掛起時間",
            0x1350,
            0x1F,
            ms_steps(0, 7000, 500, 500),
        ),
        bits(
            "private_hang_time",
            DMR,
            "個呼掛起時間",
            0x1351,
            0x1F,
            ms_steps(0, 7000, 500, 500),
        ),
        bits(
            "tx_preamble",
            DMR,
            "導入延時",
            0x1370,
            0xFF,
            ms_steps(50, 500, 10, 10),
        ),
        bits(
            "dtmf_tone_time",
            DMR,
            "DTMF音長時間",
            0x1371,
            0xFF,
            ms_steps(50, 500, 10, 10),
        ),
        bits(
            "dtmf_gap_time",
            DMR,
            "DTMF碼間隔時間",
            0x1372,
            0xFF,
            ms_steps(0, 500, 10, 10),
        ),
        bits(
            "dtmf_volume",
            DMR,
            "DTMF音量(本機)",
            0x1373,
            0x0F,
            off_and(labelled(1..13, |v| v.to_string(), same)),
        ),
        bits("dtmf", DMR, "DTMF開關", 0x1363, 0x01, off_on()),
    ]);
    let before_text = s.len();
    s.extend([
        bits("gps", DMR, "GPS", 0x137A, 0x01, off_on()),
        bits(
            "gps_interval",
            DMR,
            "GPS上報間隔",
            0x1353,
            0xFF,
            off_and(
                labelled((10..=50).step_by(10), |s| format!("{s}S"), |s| s / 10)
                    .into_iter()
                    .chain(labelled(1..251, |m| format!("{m}Min"), |m| m + 5))
                    .collect(),
            ),
        )
        .depending_on(&["gps"]),
        bits(
            "mandown_report_interval",
            DMR,
            "倒放報警上報間隔",
            0x1352,
            0xFF,
            off_and(labelled(1..256, |v| format!("{v}min"), same)),
        ),
        bits(
            "mandown_angle",
            DMR,
            "倒放角度",
            0x1358,
            0xFF,
            off_and(labelled(1..91, |v| format!("{v}°"), same)),
        ),
        bits(
            "mandown_duration",
            DMR,
            "倒放持續時間",
            0x1359,
            0xFF,
            off_and(labelled(1..256, |v| format!("{v}s"), same)),
        ),
        // ---- 加密 (keys are edited on the 報警與加密 tab)
        bits(
            "enc_level",
            ENC,
            "加密等級",
            0x122E,
            0x07,
            choices(&["無", "低", "中", "高", "增強"], 0),
        )
        .noted("中／高等級的密鑰值須在各台對講機設定一致才能互通"),
    ]);
    // ---- 菜單選項: one byte per menu group (0xAE-0xBB), one bit per entry, 1 = shown
    for (group, offset, entries) in MENU_GROUPS {
        for (bit, label) in *entries {
            s.push(bits(
                &format!("menu_{offset:02x}_{bit}"),
                MENU,
                &format!("{group} › {label}"),
                *offset,
                1 << bit,
                hide_show(),
            ));
        }
    }
    let mut out: Vec<RadioSetting> = s.into_iter().map(RadioSetting::Bits).collect();
    // The DTMF code sits right after the DTMF switch, as in the Python table.
    out.insert(
        before_text,
        RadioSetting::Text(TextSetting {
            key: "dtmf_code".into(),
            page: DMR,
            label: "DTMF碼".into(),
            offset: 0x1364,
            size: 11,
            allowed: "0123456789ABCD*#",
            note: "本機的 DTMF 識別碼，最多 11 碼（舊軟體預設 1234）",
        }),
    );
    out
}

/// Every setting, in the order of the legacy CPS pages.
pub static SETTINGS: LazyLock<Vec<RadioSetting>> = LazyLock::new(build_settings);

/// Same order as the legacy CPS navigation tree.
pub const PAGE_ORDER: [&str; 8] = [BASIC, MENU, TONE, LED, KEYS, MIC, DMR, ENC];

pub fn pages() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for s in SETTINGS.iter() {
        if !seen.contains(&s.page()) {
            seen.push(s.page());
        }
    }
    seen.sort_by_key(|p| {
        PAGE_ORDER
            .iter()
            .position(|o| o == p)
            .unwrap_or(PAGE_ORDER.len())
    });
    seen
}

pub fn by_page(page: &str) -> Vec<&'static RadioSetting> {
    SETTINGS.iter().filter(|s| s.page() == page).collect()
}

pub fn by_key(key: &str) -> Option<&'static RadioSetting> {
    SETTINGS.iter().find(|s| s.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_leave_neighbours_alone() {
        let Some(RadioSetting::Bits(vox)) = by_key("vox_level") else {
            panic!("vox_level")
        };
        let image = vec![0xF0u8; 0x200];
        let out = vox.with_raw(&image, 5).unwrap();
        assert_eq!(out[0xA6], 0xF5);
        assert_eq!(vox.display(&out), "6");
        assert!(vox.with_raw(&image, 16).is_err());
        assert_eq!(vox.raw(&image[..0xA6]), None);
    }

    #[test]
    fn text_settings_validate_characters() {
        let Some(RadioSetting::Text(code)) = by_key("dtmf_code") else {
            panic!("dtmf_code")
        };
        let image = vec![0u8; 0x1400];
        let out = code.with_text(&image, " 12AB#* ").unwrap();
        assert_eq!(code.text(&out).as_deref(), Some("12AB#*"));
        assert_eq!(
            code.with_text(&image, "12E").unwrap_err().0,
            "DTMF碼：不可使用「E」（只能用 0123456789ABCD*#）"
        );
    }
}
