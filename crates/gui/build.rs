//! Embeds the app icon into the Windows executable, so Explorer and the taskbar show it.

fn main() {
    println!("cargo:rerun-if-changed=../../icons/clouddirstat.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../icons/clouddirstat.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the Windows icon: {error}");
        }
    }
}
