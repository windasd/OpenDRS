# OpenDRS — Rust 重寫（實驗分支 `rust-rewrite`）

把 Python 版（`src/opendrs`）逐模組移植到 Rust，目標是 **Windows、macOS、Linux 都能用**。
Python 版保留在同一個分支當「標準答案」：Rust 版的每個結果都和它比對，必須逐位元組相同。

## 現況：第一、二階段完成（除 GUI 外的功能都已移植）

| crate | 內容（對應的 Python 模組） |
|---|---|
| `opendrs-core` | 名稱編碼 `textcodec`、記錄 `records`、裝置資訊 `device`、`.F136` 讀寫 `f136`、傳輸資料流 `codeplug`、寫頻協定與序列埠 `radio_link`、模擬對講機 `radio_sim`；頻道欄位 `channel_fields`、對講機設定 `radio_settings`、分區／頻道增刪移動 `layout`、聯絡人刪除與排序 `contact_edit`、接收群組 `rxgroup`、掃描列表 `scanlist`、緊急系統／加密／GPS `alarm_enc`、快捷短信 `quickmsg`、名稱清理 `cleanup`、專案差異 `diff`、復原／重做 `history`、頻道 CSV `channels_csv`、設定檔 `settings`、備份清單 `backups`、資料夾位置 `paths`；以及重現 Python 行為的 `pystr`（`str` 方法、`int()`、`Decimal`、`repr`，Unicode 表由 Python 產生）與 `pycsv`（`csv` 模組） |
| `opendrs-firmware` | IAP 韌體升級 `iap`／`iap_sim`、錄音讀取 `recording`／`recording_sim`（含 AMBE+2 解碼呼叫與 WAV）、韌體檢查 `firmware_check`、錄音讀取修補 `fwpatch`、支援機型 `hardware` |
| `opendrs-contacts` | RadioID.net 資料庫下載／快取／搜尋 `radioid`、匯入成聯絡人 `contact_import`、聯絡人 CSV `contacts_csv` |
| `opendrs-cli` | 指令列工具 `opendrs-rs`（見下） |

尚未移植：GUI（第三階段）、列印、軟體更新。

指令列工具 `opendrs-rs`：

```
opendrs-rs info     專案.F136              裝置資訊、數量、問題
opendrs-rs channels 專案.F136 [--codec big5|gbk|ascii]
opendrs-rs contacts 專案.F136 [--codec …]
opendrs-rs check    專案.F136              重新輸出是否逐位元組相同、能否寫入
opendrs-rs ports                          列出序列埠
opendrs-rs read  --port COM4 -o 讀到的.F136              讀取（自動備份到資料夾的 backups）
opendrs-rs write --port COM4 專案.F136 --yes            寫入：先讀取並備份，機型／頻段不符就中止
opendrs-rs clock --port COM4 --yes                      校正對講機時鐘
```

`--port sim`（可加 `--sim-from 專案.F136` 當模擬對講機的內容）不接實機也能試整個流程。
**Rust 版的讀寫、韌體升級、錄音讀取都還沒在實機驗證**；實機請先用 Python 版。

## 跨平台

| | Windows | macOS | Linux |
|---|---|---|---|
| 資料夾（設定、備份、錄音、RadioID 快取） | `%APPDATA%\OpenDRS`（與 Python 版共用） | `~/Library/Application Support/OpenDRS` | `$XDG_CONFIG_HOME/OpenDRS`，預設 `~/.config/OpenDRS` |
| 序列埠名稱 | `COM4` | `/dev/cu.usbserial-…` | `/dev/ttyUSB0`（使用者需在 `dialout` 群組） |
| 錄音解碼（DVSI `ambep2.exe`，只呼叫、不附帶） | ✓ | 只存原始資料 | 只存原始資料 |
| 廠商 `IAP+Flash.exe` 預設位置 | ✓ | — | — |

- 環境變數 `OPENDRS_DATA_DIR` 可指定資料夾（測試、可攜式使用）。
- 設定檔是 UTF-8 JSON；Rust 版讀到自己不認得的欄位會原樣保留，Windows 上與 Python 版共用同一個檔案也不會互相刪掉設定。
- Linux 列出序列埠需要 libudev（`opendrs-core` 預設功能 `libudev`；建置需要 `libudev-dev` 與 `pkg-config`）。
  不需要列埠的程式可用 `default-features = false`（`opendrs-firmware` 就是這樣）。
- RadioID 下載用 rustls 加作業系統的憑證庫（`ureq` + `platform-verifier`），三個平台行為相同。

## 建置與測試

```
cd rust
cargo build --release          # target/release/opendrs-rs(.exe)
cargo test --workspace         # 182 項測試
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

在 Windows 上檢查另外兩個平台能否編譯（不含需要 C 編譯器的 `opendrs-contacts`）：

```
rustup target add x86_64-unknown-linux-gnu aarch64-apple-darwin
cargo clippy --target x86_64-unknown-linux-gnu --no-default-features -p opendrs-core -p opendrs-cli -p opendrs-firmware --all-targets -- -D warnings
cargo clippy --target aarch64-apple-darwin     --no-default-features -p opendrs-core -p opendrs-cli -p opendrs-firmware --all-targets -- -D warnings
```

GitHub Actions（`.github/workflows/rust.yml`）在 Windows、Ubuntu、macOS 上各跑一次完整的格式、clippy、測試，
另有一個工作用 Python 3.13.15 重新產生所有比對答案，確認提交的版本是最新的。

## 怎麼確認和 Python 版一樣

1. **Python 標準答案**：`rust/tools/make_parity*.py` 用 Python 版跑過所有測試資料與上千個邊界案例，
   把結果存成 JSON；Rust 的測試逐欄比對，任何差異都列出來。

   | 產生器 | 答案檔 | 涵蓋 |
   |---|---|---|
   | `make_parity.py` | `opendrs-core/tests/parity.json` | 專案解析、名稱編碼、頻率換算、傳輸資料流 |
   | `make_parity_model.py` | `opendrs-core/tests/parity_model.json` | 4 份專案 × 約 650 個編輯案例：頻道 CSV 匯出與 22 種匯入情境、分區／頻道增刪移動、聯絡人刪除、各種清單、對講機設定、名稱清理、專案差異 |
   | `make_parity_firmware.py` | `opendrs-firmware/tests/parity.json` | IAP 影像與 21 種升級情境、錄音協定與 WAV、韌體檢查、修補 |
   | `make_parity_contacts.py` | `opendrs-contacts/tests/parity.json` | RadioID CSV、搜尋、下載情境、匯入聯絡人、聯絡人 CSV |
   | `make_parity_contacts.py --tables` | `opendrs-core/src/pystr_tables.rs` | Python 的 Unicode 表（空白、大小寫折疊、數字、可列印字元） |
   | `make_codec_tables.py` | `opendrs-core/src/codec_tables.rs` | Python 的 `big5`／`gbk` 編碼表（名稱欄位的編碼、解碼都用它） |

   Python 版有改動時，從工作樹根目錄重新產生（以 parity.json 為例）：

   ```
   set PYTHONPATH=src
   set PYTHONUTF8=1
   ..\opendrs\.venv\Scripts\python rust\tools\make_parity.py > rust\opendrs-core\tests\parity.json
   ```

2. **實機擷取重播**（`opendrs-core/tests/capture.rs`）：2026-10-08 舊寫頻軟體讀寫實機的 USB 擷取，Rust 版送出的每個位元組
   都必須和舊軟體相同，並能正確解讀對講機的每個回覆。IAP 升級也和廠商工具的傳送內容逐位元組比對。
3. **模擬對講機／開機程式／錄音機**：寫入後讀回、身分不符拒絕、漏收確認、校驗和錯誤、沒有回應等錯誤情境。

## 移植時順便修正的 Python 版問題

比對時發現的 Python 版問題，兩邊一起修（`main` 分支的工作樹也已套用、尚未提交）：

- `mhz_to_hz`：超過 28 位有效數字時會被四捨五入；`1E999999999` 這類輸入會建出十億位的整數而卡住，
  `1E999999999999999999` 會丟出未處理的例外。現在精確換算，超出 64 位元的值視為無效頻率。
- 頻道 CSV 的選項欄填 `²` 之類的上標數字時，錯誤訊息會是英文的 `int()` 例外；改用 `isdecimal()`。
- `settings.json` 內容是 JSON 陣列或數字時程式啟動會崩潰；現在改用預設值。
- 備份檔名接受全形數字、結尾換行；現在只接受 `save_backup` 寫出的格式。
- 檔案開頭是 UTF-16 BOM、內容卻不是合法 UTF-16 時匯入 CSV 會崩潰（Rust 版則默默以替代字元解碼）；
  現在兩邊都改試其他編碼。

## 已知差異

- 錯誤訊息裡由作業系統或函式庫產生的部分（序列埠、檔案、網路錯誤的說明）用字不同；程式自己的訊息相同。
- 錄音：`ambep2.exe` 的錯誤輸出以 WHATWG Big5 而非 cp950 解碼；超過 20 位數的錄音編號視為無效。
- RadioID：`meta.json` 的下載時間無法解析時，Python 版會崩潰，Rust 版改用檔案修改時間；下載沒有逐次讀取的
  逾時（Python 版 60 秒），改為整個檔案 30 分鐘的上限，且可隨時取消（`download_cancellable`）；
  代理伺服器只看環境變數，不讀 Windows 登錄。
- 設定檔：Rust 版先寫暫存檔再改名（寫到一半當機不會留下半個檔），無法解析的設定檔會另存為 `settings.json.bad`
  再改用預設值；Python 版直接覆寫。
- 頻道 CSV 的引用欄位寫超過 4300 位數的編號時，Python 版回報 `int()` 的英文限制訊息，Rust 版回報「編號不存在」。

## 下一步：第三階段 GUI（egui／eframe）

- 框架：**egui + eframe**——目前下載量最大的純 Rust GUI，三個平台都只需 Rust 工具鏈，單一執行檔。
- 中文字型：不附字型檔，啟動時從系統載入（Windows `msjh.ttc`／`mingliu.ttc`、macOS `PingFang.ttc`、
  Linux 透過 fontconfig 找 Noto Sans CJK），找不到時提示使用者。
- 頻道表、聯絡人表用 `egui_extras::TableBuilder`；`channels_csv::FIELDS` 的 `text`／`apply` 直接當表格欄位的顯示與編輯，
  每次專案改變時建一個 `channels_csv::View`（引用欄位的名稱只解碼一次，表格每格每幀都能直接用）。
- 讀寫頻、韌體升級、錄音下載在背景執行緒進行，以 `History` 做復原／重做。
