# OpenDRS（Rust 版，分支 `rust-rewrite`）

Kydera DR-8500UV／HORA DH-98VU 對講機寫頻軟體的 Rust 重寫，Windows、macOS、Linux 都能用：
視窗程式 `opendrs`（egui）與指令列工具 `opendrs-rs`。原本的 Python 版在 `main` 分支。

**Rust 版的讀寫頻、韌體升級、錄音讀取都還沒在實機驗證；操作實機請先用 Python 版。**
工具列的連接埠選單最後一項「模擬對講機（試用）」不接實機也能試讀取、寫入。

## 執行

```
cd rust
cargo run --release -p opendrs-gui -- 專案.F136     # 視窗程式（建好後在 target/release/opendrs(.exe)）
cargo run --release -p opendrs-cli -- --help          # 指令列工具 opendrs-rs
cargo xtask dist                                      # 打包成 dist/OpenDRS-<版本>-<平台>.zip／.tar.gz
```

需要 Rust 1.85 以上；Linux 另需 `libudev-dev` 與 `pkg-config`。

## 內容

| 位置 | 說明 |
|---|---|
| `rust/` | Cargo workspace：核心、韌體、聯絡人、指令列、視窗程式、打包工具 |
| `rust/README.md` | 架構、跨平台、建置與測試、打包與自我檢查、與 Python 版的比對方式和已知差異 |
| `rust/opendrs-gui/PORTING.md` | 視窗程式與 Python 版 GUI 的對照與刻意不同之處 |
| `rust/fixtures/` | 測試資料（專案檔、實機擷取、韌體與錄音樣本） |
| `.github/workflows/rust.yml` | 三個平台的格式、clippy、測試、打包，以及比對答案是否最新 |
