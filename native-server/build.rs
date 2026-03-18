// ─────────────────────────────────────────────────────────────────────────────
// build.rs — Propagate OEM profile detection from native-sovd
//
// Mirrors the detection logic in native-sovd/build.rs so that main.rs
// can use the same `has_oem_<name>` cfg flags for profile selection.
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    let oem_src = std::path::Path::new("../native-sovd/src");

    println!("cargo:rerun-if-changed=../native-sovd/src/");

    // Declare known OEM cfg flags for check-cfg lint
    println!("cargo::rustc-check-cfg=cfg(has_oem_mbds)");

    if let Ok(entries) = std::fs::read_dir(oem_src) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with("oem_")
                && name_str.ends_with(".rs")
                && name_str != "oem_sample.rs"
            {
                let cfg_name = name_str.trim_end_matches(".rs");
                let cfg_flag = format!("has_{cfg_name}");
                println!("cargo:rustc-cfg={cfg_flag}");
            }
        }
    }
}
