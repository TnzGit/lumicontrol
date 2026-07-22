#[cfg(windows)]
fn main() {
    use std::env;
    use std::fs;
    use std::path::PathBuf;

    let version = env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    let mut numeric = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    numeric.resize(3, 0);

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir
        .join("../lumi-ui/src-tauri/icons/icon.ico")
        .to_string_lossy()
        .replace('\\', "/");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("lumi-agent.rc");
    let resource = format!(
        r#"#include <windows.h>

1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "LumiControl\0"
      VALUE "FileDescription", "LumiControl background agent\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "lumi-agent\0"
      VALUE "OriginalFilename", "lumi-agent.exe\0"
      VALUE "ProductName", "LumiControl\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#,
        major = numeric[0],
        minor = numeric[1],
        patch = numeric[2],
    );
    fs::write(&output, resource).expect("write Lumi Agent Windows resource");

    println!("cargo:rerun-if-changed={icon}");
    embed_resource::compile_for(&output, ["lumi-agent"], embed_resource::NONE)
        .manifest_required()
        .expect("compile Lumi Agent Windows resource");
}

#[cfg(not(windows))]
fn main() {}
