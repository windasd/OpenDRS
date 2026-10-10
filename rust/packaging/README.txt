OpenDRS {version}（Rust 版）
Kydera DR-8500UV／HORA DH-98VU 對講機寫頻軟體
https://github.com/windasd/OpenDRS

內容
  OpenDRS（Windows：OpenDRS.exe；macOS：OpenDRS.app；Linux：opendrs）  視窗程式
  opendrs-rs                                                         指令列工具（opendrs-rs --help）
  licenses/                                                          第三方元件授權

注意：Rust 版的讀寫頻、韌體升級、錄音讀取尚未在實機驗證；操作實機請先用 Python 版。
工具列的連接埠選單最後一項「模擬對講機（試用）」不接實機也能試讀取、寫入。

各平台
  Windows  解壓縮後執行 OpenDRS.exe。
  macOS    程式沒有經過 Apple 簽署。第一次開啟被擋下後，到「系統設定 → 隱私權與安全性」按「仍要打開」；
           或在終端機執行 xattr -dr com.apple.quarantine OpenDRS.app 再開啟。
           寫頻線的序列埠名稱是 /dev/cu.usbserial-…（依驅動程式而定）。
  Linux    執行 ./opendrs。使用者要在 dialout 群組才能開啟序列埠（sudo usermod -aG dialout $USER，重新登入）。
           要加入應用程式選單：把 opendrs 放到 PATH 中的資料夾，
           opendrs.desktop 複製到 ~/.local/share/applications/，opendrs.png 複製到 ~/.local/share/icons/。

  中文字型使用系統字型（Windows 微軟正黑體、macOS 蘋方、Linux Noto Sans CJK 或文泉驛）；
  找不到時可用環境變數 OPENDRS_FONT 指定字型檔。
  匯出 PDF 需要已安裝 Microsoft Edge 或 Google Chrome；沒有時可用「預覽列印」在瀏覽器中另存 PDF。
  畫面閃爍或無法顯示時，可用環境變數 WGPU_BACKEND 換一種繪圖方式再開啟（Windows 預設 dx12，可改 vulkan 或 gl；
  例如在命令提示字元執行 set WGPU_BACKEND=gl 後再執行 OpenDRS.exe）。
  錄音轉成 WAV 需要原廠附帶的 ambep2.exe（只限 Windows），本程式不附帶。

自我檢查
  在終端機執行（macOS：OpenDRS.app/Contents/MacOS/OpenDRS）：
    OpenDRS --self-test 報告.json
  檢查中文字型、繪圖介面、所有畫面、模擬對講機讀寫、序列埠、TLS、匯出 PDF，結果寫在報告中；
  全部通過時結束碼為 0。不會讀寫你的設定，也不會開啟任何序列埠。

資料夾（設定、備份、錄音、RadioID 快取）
  Windows  %APPDATA%\OpenDRS（與 Python 版共用）
  macOS    ~/Library/Application Support/OpenDRS
  Linux    ~/.config/OpenDRS
  環境變數 OPENDRS_DATA_DIR 可指定其他位置（例如可攜式使用）。
