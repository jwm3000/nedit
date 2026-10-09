// Embeds the nEdit icon into the Windows executable (shown in Explorer and the taskbar).
fn main() {
    println!("cargo:rerun-if-changed=assets/nedit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/nedit.ico");
        res.set("ProductName", "nEdit");
        res.set("FileDescription", "nEdit – LaTeX Studio");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Windows-Icon konnte nicht eingebettet werden: {e}");
        }
    }
}
