// Embeds the icon, full file details and an application manifest into the Windows .exe.
// Complete details and a manifest (normal user rights, no admin) make the program look like
// the ordinary app it is to Windows and to antivirus heuristics.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("assets/icon.ico", "1");
        res.set("FileDescription", "Program Prison");
        res.set("ProductName", "Program Prison");
        res.set("CompanyName", "Program Prison");
        res.set("LegalCopyright", "Program Prison contributors");
        res.set("OriginalFilename", "Program Prison.exe");
        res.set("InternalName", "programprison");
        res.set("Comments", "Blocks distracting websites and programs on a timer or schedule. https://github.com/demonztarz/ProgramPrison");
        res.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="ProgramPrison" version="1.0.0.0"/>
  <description>Program Prison</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#,
        );
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon and file details: {e}");
        }
    }
}
