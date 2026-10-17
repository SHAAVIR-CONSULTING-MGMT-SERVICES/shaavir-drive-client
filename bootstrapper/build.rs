// build.rs - turn oem.json into compile-time constants (src/tenant.rs includes the result) and, on
// Windows, embed the application manifest, version info and icon.
//
// oem.json is the per-tenant file the private generator writes (client-build/gen-oem.py); in the public
// repo it sits next to this folder. Override with SDRIVE_OEM_JSON. Keys read:
//   product_name, vendor, updates_host, executable, shortname, version, guids.GUID_MSI_UPGRADE_CODE,
//   bootstrapper.manifest_pubkey, bootstrapper.min_windows_build
// A release build needs a manifest key: without one the build fails unless SDRIVE_DEV_BUILD=1, and such a
// dev build refuses to install anything (see src/app.rs).
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn s<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str()
}

fn lit(x: &str) -> String {
    format!("{:?}", x)
}

fn main() {
    println!("cargo:rerun-if-env-changed=SDRIVE_OEM_JSON");
    println!("cargo:rerun-if-env-changed=SDRIVE_DEV_BUILD");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.ico");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let oem_path = env::var("SDRIVE_OEM_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("..").join("oem.json"));
    println!("cargo:rerun-if-changed={}", oem_path.display());
    let dev = env::var("SDRIVE_DEV_BUILD").map(|v| v == "1").unwrap_or(false);
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let oem: Option<serde_json::Value> = fs::read_to_string(&oem_path)
        .ok()
        .map(|t| serde_json::from_str(&t).expect("oem.json is not valid JSON"));
    let get = |path: &[&str], default: &str| -> String {
        oem.as_ref().and_then(|o| s(o, path)).unwrap_or(default).to_string()
    };
    let product = get(&["product_name"], "Example Drive");
    let vendor = get(&["vendor"], "Example");
    let host = get(&["updates_host"], "updates.example.invalid");
    let exe = get(&["executable"], "exampledrive");
    let shortname = get(&["shortname"], "ExampleDrive");
    let floor = get(&["version"], "0.0.0.0");
    let upgrade = get(&["guids", "GUID_MSI_UPGRADE_CODE"], "");
    let key = get(&["bootstrapper", "manifest_pubkey"], "");
    let min_build: u32 = oem
        .as_ref()
        .and_then(|o| o.get("bootstrapper"))
        .and_then(|b| b.get("min_windows_build"))
        .and_then(|v| v.as_u64())
        .unwrap_or(19045) as u32;

    if target_os == "windows" && !dev {
        assert!(oem.is_some(), "no oem.json at {} (set SDRIVE_OEM_JSON)", oem_path.display());
        assert!(!key.is_empty(), "oem.json has no bootstrapper.manifest_pubkey; set SDRIVE_DEV_BUILD=1 for a build that cannot install");
        assert!(!upgrade.is_empty(), "oem.json has no guids.GUID_MSI_UPGRADE_CODE");
    }
    assert!(
        host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'),
        "updates_host is not a plain host name: {host}"
    );

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let code = format!(
        "pub const PRODUCT_NAME: &str = {};\npub const VENDOR: &str = {};\npub const UPDATES_HOST: &str = {};\n\
         pub const APP_EXECUTABLE: &str = {};\npub const SHORTNAME: &str = {};\npub const VERSION_FLOOR: &str = {};\n\
         pub const MSI_UPGRADE_CODE: &str = {};\npub const MANIFEST_PUBKEY: &str = {};\npub const MIN_WINDOWS_BUILD: u32 = {};\n\
         pub const DEV_BUILD: bool = {};\n",
        lit(&product), lit(&vendor), lit(&host), lit(&exe), lit(&shortname), lit(&floor),
        lit(&upgrade), lit(&key), min_build, dev || key.is_empty()
    );
    fs::write(out.join("tenant.rs"), code).unwrap();

    if target_os == "windows" {
        let ico = manifest_dir.join("app.ico");
        let file_desc = format!("{product} Setup");
        let mut rc = String::new();
        rc.push_str("#pragma code_page(65001)\n");
        rc.push_str(&format!("1 24 \"{}\"\n", esc(&manifest_dir.join("app.manifest"))));
        if ico.is_file() {
            rc.push_str(&format!("1 ICON \"{}\"\n", esc(&ico)));
        }
        rc.push_str(&format!(
            "1 VERSIONINFO\nFILEVERSION 1,0,0,0\nPRODUCTVERSION 1,0,0,0\nFILEOS 0x40004\nFILETYPE 0x1\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904B0\"\n  BEGIN\n   VALUE \"CompanyName\", \"{v}\"\n   VALUE \"FileDescription\", \"{d}\"\n   VALUE \"FileVersion\", \"1.0.0.0\"\n   VALUE \"InternalName\", \"{n}Setup\"\n   VALUE \"OriginalFilename\", \"{n}Setup.exe\"\n   VALUE \"ProductName\", \"{p}\"\n   VALUE \"ProductVersion\", \"1.0.0.0\"\n  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
            v = rcq(&vendor), d = rcq(&file_desc), n = rcq(&shortname), p = rcq(&product)
        ));
        let rc_path = out.join("app.rc");
        fs::write(&rc_path, rc).unwrap();
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_required()
            .expect("embedding resources failed");
    }
}

fn esc(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

fn rcq(x: &str) -> String {
    x.replace('"', "\"\"")
}
