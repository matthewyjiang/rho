fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set("FileDescription", "Rho Coding Agent");
    resource.set("ProductName", "Rho");
    resource.set("OriginalFilename", "rho.exe");
    resource.set("CompanyName", "Rho contributors");
    resource.set("LegalCopyright", "Copyright (c) Rho contributors");
    resource.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
    );

    if let Err(err) = resource.compile() {
        panic!("failed to compile Windows resources: {err}");
    }
}
