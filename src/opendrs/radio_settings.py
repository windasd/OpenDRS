"""General radio settings stored as bit fields in the settings image.

Each field is declared once (page, label, offset, bit mask, and either a list of
choices or a numeric range). Values are read and written through the mask only, so
neighbouring settings that share a byte are never disturbed.

The offsets and encodings were mapped by driving the legacy CPS (legacy_probe/):
every option of every control was selected, the project saved and compared with a
baseline, so each choice below is the exact raw value the legacy CPS writes.
"""

from __future__ import annotations

from dataclasses import dataclass, field


class SettingError(ValueError):
    pass


@dataclass(frozen=True)
class Setting:
    key: str
    page: str
    label: str
    offset: int
    mask: int = 0xFF
    choices: tuple[tuple[str, int], ...] = ()  # (label, raw value within the mask)
    minimum: int = 0
    maximum: int = 0
    scale: int = 1  # shown value = raw * scale (e.g. TOT stored in tens of seconds)
    unit: str = ""
    size: int = 1  # bytes, little-endian, for multi-byte numbers
    note: str = ""
    depends: tuple[str, ...] = field(default=())

    @property
    def shift(self) -> int:
        return (self.mask & -self.mask).bit_length() - 1

    @property
    def is_choice(self) -> bool:
        return bool(self.choices)

    def raw(self, image: bytes) -> int | None:
        end = self.offset + self.size
        if len(image) < end:
            return None
        value = int.from_bytes(image[self.offset:end], "little")
        return (value & self.mask) >> self.shift

    def display(self, image: bytes) -> str:
        raw = self.raw(image)
        if raw is None:
            return "—"
        if self.is_choice:
            return next((label for label, v in self.choices if v == raw), f"未知值 {raw}")
        return f"{raw * self.scale}{self.unit}"

    def with_raw(self, image: bytes, raw: int) -> bytes:
        end = self.offset + self.size
        if len(image) < end:
            raise SettingError("設定區太短，無法寫入此設定")
        limit = self.mask >> self.shift
        if not 0 <= raw <= limit:
            raise SettingError(f"{self.label}：值超出範圍")
        if self.is_choice and raw not in {v for _, v in self.choices}:
            raise SettingError(f"{self.label}：不是有效的選項")
        if not self.is_choice and not self.minimum <= raw * self.scale <= self.maximum:
            raise SettingError(f"{self.label}：必須介於 {self.minimum} 與 {self.maximum}{self.unit}")
        old = int.from_bytes(image[self.offset:end], "little")
        new = (old & ~self.mask) | (raw << self.shift)
        return image[:self.offset] + new.to_bytes(self.size, "little") + image[end:]

    def with_value(self, image: bytes, value: int) -> bytes:
        """Numeric settings: ``value`` in display units (must be a multiple of scale)."""
        if value % self.scale:
            raise SettingError(f"{self.label}：必須是 {self.scale} 的倍數")
        return self.with_raw(image, value // self.scale)


@dataclass(frozen=True)
class TextSetting:
    """A NUL-padded ASCII text field (e.g. the DTMF code)."""

    key: str
    page: str
    label: str
    offset: int
    size: int
    allowed: str = ""  # permitted characters ("" = any printable ASCII)
    note: str = ""
    depends: tuple[str, ...] = field(default=())

    def text(self, image: bytes) -> str | None:
        chunk = image[self.offset:self.offset + self.size]
        if len(chunk) < self.size:
            return None
        return chunk.split(bytes(1), 1)[0].decode("ascii", errors="replace")

    def with_text(self, image: bytes, text: str) -> bytes:
        if len(image) < self.offset + self.size:
            raise SettingError("設定區太短，無法寫入此設定")
        text = text.strip()
        if len(text) > self.size:
            raise SettingError(f"{self.label}：最多 {self.size} 個字元")
        bad = [ch for ch in text if (self.allowed and ch not in self.allowed) or not 0x20 <= ord(ch) < 0x7F]
        if bad:
            hint = f"（只能用 {self.allowed}）" if self.allowed else ""
            raise SettingError(f"{self.label}：不可使用「{bad[0]}」{hint}")
        value = text.encode("ascii").ljust(self.size, bytes(1))
        return image[:self.offset] + value + image[self.offset + self.size:]


def choices(*labels: str, start: int = 0) -> tuple[tuple[str, int], ...]:
    return tuple((label, i) for i, label in enumerate(labels, start=start))


def labelled(fmt: str, values: range, raw=lambda v: v) -> tuple[tuple[str, int], ...]:  # noqa: ANN001
    return tuple((fmt.format(v), raw(v)) for v in values)


OFF_ON = choices("Off", "On")
HIDE_SHOW = choices("隱藏", "顯示")
BASIC = "基本設置"
MENU = "菜單選項"
TONE = "提示音"
LED = "指示燈"
KEYS = "自定義按鍵"
MIC = "MIC增益"
DMR = "DMR服務項目"
ENC = "加密"
KEY_FUNCTIONS = (
    *choices("未定義", "高低功率切換", "背景燈開/關", "鍵盤鎖", "聲控開關", "區域切換", "掃描開/關", "掃描模式",
             "中繼/脫網", "緊急開關", "加密開/關", "聯系人", "短信", "對講機激活", "對講機檢測", "對講機遙斃",
             "遠程監聽", "監聽", "永久監聽"),
    ("DTMF開關", 49), ("漫游開關", 52), ("中轉開關", 53), ("中轉監聽開關", 54), ("單雙守候開關", 63),
)
TONE_PICK = (("Off", 0), *labelled("{}", range(1, 6)))
# (group, byte, [(bit, entry)]) — read by the firmware as one 0x12-byte block from
# 0x3C00AE (FUN_0304a580). 附件 bits 1-2 (手電筒/收音機) are greyed out in the legacy
# CPS for this model and left alone.
MENU_GROUPS: tuple[tuple[str, int, tuple[tuple[int, str], ...]], ...] = (
    ("聯系人", 0xAE, ((0, "聯系人列表"), (1, "新建聯系人"), (2, "手動撥號"))),
    ("增強業務", 0xBA, ((0, "檢測"), (1, "呼叫提示"), (2, "遠程監聽"), (3, "遙斃"), (4, "激活"))),
    ("短信", 0xAF, ((0, "寫信息"), (1, "常用短信"), (2, "收信箱"), (3, "發信箱"), (4, "草稿箱"))),
    ("通話記錄", 0xB0, ((0, "呼出號碼"), (1, "已接來電"), (2, "未接來電"))),
    ("掃描", 0xB1, ((0, "掃描開關"), (1, "掃描列表"), (2, "掃描模式"), (3, "漫遊開關"))),
    ("區域設置", 0xB2, ((0, "區域列表"),)),
    ("對講機設置", 0xB3, ((0, "語言"), (1, "鎖鍵"), (2, "LCD背景燈"), (3, "LED指示燈"), (4, "顯示模式"),
                      (5, "聲控"), (6, "信道開關"), (7, "恢復出廠設置"))),
    ("參數設置", 0xB5, ((0, "呼叫時限"), (1, "功率設置"), (2, "中繼設置"), (3, "省電模式"), (4, "靜噪級別"),
                    (5, "寬窄帶"), (6, "繁忙開關"), (7, "信令設置"))),
    ("加密設置", 0xB4, ((0, "加密設置"),)),
    ("提示音設置", 0xB6, ((0, "情景"), (1, "按鍵"), (2, "電量"), (3, "短信"), (4, "個呼結束"), (5, "組呼結束"),
                     (6, "呼叫提示"), (7, "開機提示音"))),
    ("附件", 0xB7, ((0, "GPS"), (3, "時間"), (4, "DTMF"), (5, "手咪喇叭"))),
    ("錄音", 0xB8, ((0, "錄音設置"), (1, "錄音列表"), (2, "清空錄音"), (3, "查詢空間"))),
    ("設備信息", 0xB9, ((0, "本機個號"), (1, "接收組列表"), (2, "默認聯系人"), (3, "版本號"))),
    ("信道編輯", 0xBB, ((0, "信道編輯"),)),
)

SETTINGS: tuple[Setting | TextSetting, ...] = (
    # ---- 基本設置 (radio name and ID are on the device page)
    Setting("language", BASIC, "語言設置", 0x95, 0x10, choices("中文", "英文")),
    Setting("tot", BASIC, "限時發射", 0x134F, 0xFF,
            (("Off", 0), *labelled("{}S", range(20, 501, 10), lambda s: s // 10))),
    Setting("busy_lockout", BASIC, "繁忙鎖定開關", 0xA6, 0x80, OFF_ON),
    Setting("vox", BASIC, "聲控發射開關", 0xA6, 0x40, OFF_ON),
    Setting("vox_level", BASIC, "聲控發送靈敏度", 0xA6, 0x0F, labelled("{}", range(1, 13), lambda v: v - 1)),
    Setting("power_save", BASIC, "省電模式開關", 0x96, 0x04, OFF_ON),
    Setting("power_save_ratio", BASIC, "省電模式", 0x96, 0x18, choices("1:1", "1:2", "1:4"),
            depends=("power_save",)),
    Setting("power_save_delay", BASIC, "省電啟動時間", 0xA3, 0xFF, labelled("{}S", range(10, 61)),
            depends=("power_save",)),
    Setting("scan_mode", BASIC, "掃描模式", 0x137D, 0x03, choices("CO", "TO", "SE")),
    Setting("tail_tone", BASIC, "尾音類型", 0x1381, 0x03, choices("55Hz", "120°", "180°", "240°")),
    Setting("squelch_a", BASIC, "靜噪(A)", 0x93, 0x0F, labelled("{}", range(10))),
    Setting("squelch_b", BASIC, "靜噪(B)", 0x93, 0xF0, labelled("{}", range(10))),
    Setting("backlight", BASIC, "背景燈開關", 0x95, 0x28, (("Off", 0), ("On", 1), ("Auto", 4))),
    Setting("keylock", BASIC, "鎖鍵開關", 0x95, 0x04, OFF_ON),
    Setting("roaming", BASIC, "漫遊開關", 0x137B, 0x01, OFF_ON),
    Setting("roaming_mode", BASIC, "漫遊模式", 0x1375, 0x03, choices("自動", "手動", "強信號優先")),
    Setting("roaming_rssi", BASIC, "Rssi信號", 0x1376, 0x3F,
            labelled("-{}dBm", range(90, 126), lambda v: v - 90)),
    Setting("site_search_timer", BASIC, "站點搜索計時器", 0x1377, 0xFF, labelled("{}S", range(256))),
    Setting("site_confirm_timer", BASIC, "站點確認計時器", 0x1378, 0xFF, labelled("{}S", range(256))),
    Setting("connect_count", BASIC, "連接次數", 0x1379, 0x0F, labelled("{}", range(1, 11), lambda v: v - 1)),
    Setting("record", BASIC, "錄音設置", 0x1380, 0x03, choices("Off", "TX", "RX", "TX/RX")),
    # ---- 提示音 (picking 靜音 in the legacy CPS also rewrites both volumes; here each
    # control keeps to its own bits)
    Setting("tone_profile", TONE, "情景模式", 0xA7, 0x01, choices("標準", "靜音")),
    Setting("tone_sms", TONE, "短信提示音", 0xA8, 0x07, TONE_PICK),
    Setting("tone_private", TONE, "個呼提示音", 0xA9, 0x07, TONE_PICK),
    Setting("tone_group", TONE, "組呼提示音", 0xAA, 0x07, TONE_PICK),
    Setting("tone_key", TONE, "按鍵提示音開關", 0xAB, 0x80, OFF_ON),
    Setting("tone_key_volume", TONE, "按鍵音量", 0xAB, 0x0F, labelled("{}", range(1, 14)),
            depends=("tone_key",)),
    Setting("tone_low_battery", TONE, "低電壓提示音開關", 0xAC, 0x80, OFF_ON),
    Setting("tone_low_battery_volume", TONE, "低電壓音量", 0xAC, 0x0F, labelled("{}", range(1, 14)),
            depends=("tone_low_battery",)),
    Setting("tone_call_hang", TONE, "呼叫掛起", 0x12D8, 0x01, choices("靜音", "提示音")),
    Setting("tone_power_on", TONE, "開機提示音開關", 0x95, 0x02, OFF_ON),
    Setting("tone_roam_restart", TONE, "漫遊重啟提示音", 0x1382, 0x07, TONE_PICK),
    Setting("tone_site_lock", TONE, "站點鎖定提示音", 0x1383, 0x07, TONE_PICK),
    # ---- 指示燈: one byte, one bit per LED
    Setting("led_device", LED, "設備指示燈開關", 0xAD, 0x10, OFF_ON),
    Setting("led_tx", LED, "發射指示燈開關", 0xAD, 0x08, OFF_ON),
    Setting("led_rx", LED, "接收指示燈開關", 0xAD, 0x04, OFF_ON),
    Setting("led_scan", LED, "掃描狀態指示燈開關", 0xAD, 0x02, OFF_ON),
    Setting("led_low_battery", LED, "電池低電量指示燈開關", 0xAD, 0x01, OFF_ON),
    # ---- 自定義按鍵: P1-P3 long/short at 0xC2-0xC7 (P4-P6 rows are hidden for this model)
    Setting("key_long_time", KEYS, "長按持續時間", 0xC1, 0xFF,
            tuple((f"{(i + 1) * 0.5:.1f}S", i) for i in range(10))),
    *(Setting(f"key_p{p}_{kind}", KEYS, f"P{p}鍵 {label}", 0xC2 + 2 * (p - 1) + shift, 0xFF, KEY_FUNCTIONS)
      for p in (1, 2, 3) for kind, label, shift in (("long", "長按", 0), ("short", "短按", 1))),
    # ---- MIC增益
    Setting("mic_gain1_on", MIC, "內置麥克風增益開關1", 0xA4, 0x80, OFF_ON),
    Setting("mic_gain1", MIC, "內置麥克風增益1", 0xA4, 0x07, labelled("{}dB", range(0, 21, 4), lambda v: v // 4),
            depends=("mic_gain1_on",)),
    Setting("mic_gain2_on", MIC, "內置麥克風增益開關2", 0xA5, 0x80, OFF_ON),
    Setting("mic_gain2", MIC, "內置麥克風增益2", 0xA5, 0x3F, labelled("{}dB", range(44)),
            depends=("mic_gain2_on",)),
    # ---- DMR服務項目
    Setting("remote_monitor_time", DMR, "遠程監聽時間", 0xFAC, 0x0F,
            labelled("{}S", range(10, 121, 10), lambda s: s // 10 - 1)),
    Setting("decode_remote_monitor", DMR, "遠程監聽解碼（本機）", 0xFAD, 0x80, OFF_ON),
    Setting("decode_kill", DMR, "遙斃解碼（本機）", 0xFAD, 0x40, OFF_ON),
    Setting("decode_check", DMR, "檢測解碼（本機）", 0xFAD, 0x20, OFF_ON),
    Setting("decode_revive", DMR, "激活解碼（本機）", 0xFAD, 0x10, OFF_ON),
    Setting("call_alert", DMR, "呼叫提示", 0xFAD, 0x08, OFF_ON),
    Setting("group_hang_time", DMR, "組呼掛起時間", 0x1350, 0x1F,
            labelled("{}ms", range(0, 7001, 500), lambda ms: ms // 500)),
    Setting("private_hang_time", DMR, "個呼掛起時間", 0x1351, 0x1F,
            labelled("{}ms", range(0, 7001, 500), lambda ms: ms // 500)),
    Setting("tx_preamble", DMR, "導入延時", 0x1370, 0xFF, labelled("{}ms", range(50, 501, 10), lambda ms: ms // 10)),
    Setting("dtmf_tone_time", DMR, "DTMF音長時間", 0x1371, 0xFF,
            labelled("{}ms", range(50, 501, 10), lambda ms: ms // 10)),
    Setting("dtmf_gap_time", DMR, "DTMF碼間隔時間", 0x1372, 0xFF,
            labelled("{}ms", range(0, 501, 10), lambda ms: ms // 10)),
    Setting("dtmf_volume", DMR, "DTMF音量(本機)", 0x1373, 0x0F, (("Off", 0), *labelled("{}", range(1, 13)))),
    Setting("dtmf", DMR, "DTMF開關", 0x1363, 0x01, OFF_ON),
    TextSetting("dtmf_code", DMR, "DTMF碼", 0x1364, 11, allowed="0123456789ABCD*#",
                note="本機的 DTMF 識別碼，最多 11 碼（舊軟體預設 1234）"),
    Setting("gps", DMR, "GPS", 0x137A, 0x01, OFF_ON),
    Setting("gps_interval", DMR, "GPS上報間隔", 0x1353, 0xFF,
            (("Off", 0), *labelled("{}S", range(10, 51, 10), lambda s: s // 10),
             *labelled("{}Min", range(1, 251), lambda m: m + 5)), depends=("gps",)),
    Setting("mandown_report_interval", DMR, "倒放報警上報間隔", 0x1352, 0xFF,
            (("Off", 0), *labelled("{}min", range(1, 256)))),
    Setting("mandown_angle", DMR, "倒放角度", 0x1358, 0xFF, (("Off", 0), *labelled("{}°", range(1, 91)))),
    Setting("mandown_duration", DMR, "倒放持續時間", 0x1359, 0xFF, (("Off", 0), *labelled("{}s", range(1, 256)))),
    # ---- 加密 (keys are edited on the 報警與加密 tab)
    Setting("enc_level", ENC, "加密等級", 0x122E, 0x07, choices("無", "低", "中", "高", "增強"),
            note="中／高等級的密鑰值須在各台對講機設定一致才能互通"),
    *(  # ---- 菜單選項: one byte per menu group (0xAE-0xBB), one bit per entry, 1 = shown
        Setting(f"menu_{offset:02x}_{bit}", MENU, f"{group} › {label}", offset, 1 << bit, HIDE_SHOW)
        for group, offset, entries in MENU_GROUPS
        for bit, label in entries
    ),
)


# Same order as the legacy CPS navigation tree.
PAGE_ORDER = ("基本設置", "菜單選項", "提示音", "指示燈", "自定義按鍵", "MIC增益", "DMR服務項目", "加密")


def pages() -> list[str]:
    seen: list[str] = []
    for s in SETTINGS:
        if s.page not in seen:
            seen.append(s.page)
    return sorted(seen, key=lambda p: PAGE_ORDER.index(p) if p in PAGE_ORDER else len(PAGE_ORDER))


def by_page(page: str) -> list[Setting]:
    return [s for s in SETTINGS if s.page == page]


def by_key(key: str) -> Setting:
    for s in SETTINGS:
        if s.key == key:
            return s
    raise KeyError(key)
