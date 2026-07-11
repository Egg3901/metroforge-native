//! Embed Windows VERSIONINFO + application icon into `metroforge.exe` so
//! Explorer → Properties shows real metadata and the exe carries the same
//! icon as the installer shortcuts (`packaging/icon.ico`).
//!
//! Uses `embed-resource` (works with `cargo-xwin` cross-compiles via
//! `llvm-rc`). On non-Windows targets this is a no-op.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon_src = manifest_dir.join("../../packaging/icon.ico");
    assert!(
        icon_src.is_file(),
        "packaging/icon.ico must exist for Windows resource embedding ({})",
        icon_src.display()
    );

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let (major, minor, patch, build) = parse_version(&version);

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // Copy into OUT_DIR so llvm-rc (cargo-xwin) resolves a simple relative
    // name — absolute Unix paths get mangled when the RC preprocessor
    // rewrites separators.
    let icon_dst = out_dir.join("icon.ico");
    fs::copy(&icon_src, &icon_dst).expect("copy packaging/icon.ico into OUT_DIR");

    let rc_path = out_dir.join("metroforge.rc");
    let rc = format!(
        r#"
1 ICON "icon.ico"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},{build}
PRODUCTVERSION {major},{minor},{patch},{build}
FILEOS 0x40004
FILETYPE 0x1
{{
    BLOCK "StringFileInfo"
    {{
        BLOCK "040904B0"
        {{
            VALUE "CompanyName", "MetroForge"
            VALUE "FileDescription", "MetroForge"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "metroforge"
            VALUE "OriginalFilename", "metroforge.exe"
            VALUE "ProductName", "MetroForge"
            VALUE "ProductVersion", "{version}"
        }}
    }}
    BLOCK "VarFileInfo"
    {{
        VALUE "Translation", 0x0409, 0x04B0
    }}
}}
"#
    );

    fs::write(&rc_path, rc).expect("write metroforge.rc");
    println!("cargo:rerun-if-changed={}", icon_src.display());
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    // OUT_DIR is already on embed-resource's include path, so "icon.ico"
    // resolves next to the generated .rc.
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("embed Windows resources (icon + VERSIONINFO)");
}

fn parse_version(version: &str) -> (u16, u16, u16, u16) {
    // Accept "0.4.4", "0.4.4-alpha", "0.4.4-alpha.1" — take the leading
    // three numeric components and leave the rest as build=0.
    let numeric = version.split('-').next().unwrap_or(version);
    let mut parts = numeric.split('.').filter_map(|p| p.parse::<u16>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
