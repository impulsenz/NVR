//! On Windows: the program's name ("GAZE", as Task Manager, the taskbar and
//! the file's properties show it) and its icon.
fn main() {
    println!("cargo:rerun-if-changed=assets/gaze.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut r = winresource::WindowsResource::new();
    r.set_icon("assets/gaze.ico")
        .set("FileDescription", "GAZE")
        .set("ProductName", "GAZE")
        .set("InternalName", "GAZE")
        .set("OriginalFilename", "GAZE.exe")
        .set("LegalCopyright", "GAZE");
    // Without the resource compiler the program still builds (just without them).
    if let Err(e) = r.compile() {
        println!("cargo:warning=couldn't add GAZE's Windows name and icon: {e}");
    }
}
