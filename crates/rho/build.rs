use std::env;
use std::path::PathBuf;

use syntect::parsing::SyntaxDefinition;

fn main() {
    build_syntax_set();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
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

/// Merge the bundled PowerShell grammar into two-face's dump once per compile
/// so runtime highlighting loads a single set.
fn build_syntax_set() {
    println!("cargo:rerun-if-changed=src/tui/powershell.sublime-syntax");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("syntaxes-newlines.bin");
    let mut builder = two_face::syntax::extra_newlines().into_builder();
    let powershell = SyntaxDefinition::load_from_str(
        include_str!("src/tui/powershell.sublime-syntax"),
        /*lines_include_newline*/ true,
        None,
    )
    .expect("bundled PowerShell syntax must be valid");
    builder.add(powershell);
    let set = builder.build();
    syntect::dumps::dump_to_uncompressed_file(&set, &out)
        .unwrap_or_else(|error| panic!("failed to dump syntax set: {error}"));
}
