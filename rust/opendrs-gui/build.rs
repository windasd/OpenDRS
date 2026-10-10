//! Windows: embed the icon and the version information (Explorer › 內容 › 詳細資料) in the .exe,
//! as the PyInstaller build's version resource does.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/opendrs.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/opendrs.ico")
        .set_language(0x0404) // zh-TW
        .set("CompanyName", "windasd")
        .set("FileDescription", "OpenDRS對講機寫頻軟體")
        .set("InternalName", "opendrs")
        .set("LegalCopyright", "© windasd")
        .set("OriginalFilename", "OpenDRS.exe")
        .set("ProductName", "OpenDRS");
    if let Err(e) = resource.compile() {
        panic!("無法嵌入 Windows 圖示與版本資訊（需要 Windows SDK 的 rc.exe）：{e}");
    }
}
