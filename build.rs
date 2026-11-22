// Embeds the icon and version information into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("assets/icon.ico", "1");
        res.set("FileDescription", "Program Prison");
        res.set("ProductName", "Program Prison");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
