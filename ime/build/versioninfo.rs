// VERSIONINFO for the shipped binaries (product name and version identical in every
// file of a release, as SignPath requires). Shared by the build scripts of twime-tsf, twime-engined and twime-setup
// through include!; the version is the crate version (all crates of a release carry the same one, and
// release/build-msi.ps1 checks it against the MSI version).

/// Writes `<OUT_DIR>/versioninfo.rc` for this crate and returns its path. `dll` selects VFT_DLL instead of VFT_APP.
fn versioninfo_rc(description: &str, original_filename: &str, dll: bool) -> std::path::PathBuf {
    println!("cargo:rerun-if-changed=../../build/versioninfo.rs");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let parts: Vec<u16> = version.split('.').map(|p| p.parse().expect("crate version must be MAJOR.MINOR.PATCH")).collect();
    assert_eq!(parts.len(), 3, "crate version must be MAJOR.MINOR.PATCH: {version}");
    let numeric = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let file_type = if dll { "0x2L" } else { "0x1L" };
    let rc = format!(
        r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE {file_type}
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040404b0"
    BEGIN
      VALUE "CompanyName", "meowhoo"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{original_filename}"
      VALUE "LegalCopyright", "Copyright (c) 2026 meowhoo. MIT License."
      VALUE "OriginalFilename", "{original_filename}"
      VALUE "ProductName", "地瓜輸入法"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0404, 1200
  END
END
"#
    );
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("versioninfo.rc");
    std::fs::write(&path, rc).unwrap();
    path
}
