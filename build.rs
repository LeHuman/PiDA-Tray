//! Embeds the application icon and Explorer metadata on Windows targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "PiDA Tray");
    res.set("FileDescription", "PiDA Tray");
    res.set("OriginalFilename", "pida-tray.exe");

    if let Err(err) = res.compile() {
        if cfg!(windows) {
            panic!("failed to embed Windows resources: {err}");
        }
        // Cross-compiling from a non-Windows host: resource tooling may be absent.
        println!("cargo:warning=skipping Windows resource embedding: {err}");
    }
}
