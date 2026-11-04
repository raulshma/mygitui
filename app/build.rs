// Windows-only resource embedding: bakes the app icon and product/file
// version metadata into mygitui.exe so Explorer, the taskbar, and shortcuts
// show the real icon instead of the default Rust binary icon. Guarded on the
// *target* OS (not the host) so Linux/macOS builds and cross-compiles skip it
// entirely — windres/rc only exists where the exe is actually linked.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo always sets CARGO_PKG_VERSION");
    let description =
        std::env::var("CARGO_PKG_DESCRIPTION").expect("cargo always sets CARGO_PKG_DESCRIPTION");

    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/icon.ico");
    res.set("FileDescription", &description);
    res.set("ProductName", "MyGitUI");
    res.set("OriginalFilename", "mygitui.exe");
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.compile().expect("failed to compile Windows resources");
}
