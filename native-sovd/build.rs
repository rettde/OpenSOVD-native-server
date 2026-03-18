// ─────────────────────────────────────────────────────────────────────────────
// build.rs — Auto-detect proprietary OEM profile files
//
// Scans `src/` for files matching `oem_*.rs` (excluding `oem_sample.rs`)
// and emits `cargo:rustc-cfg=has_oem_<name>` for each one found.
//
// This allows proprietary OEM profiles to be compiled automatically when
// the source file is present, without requiring explicit Cargo feature flags.
//
// Example:
//   src/oem_mbds.rs   present → cfg(has_oem_mbds)   is set
//   src/oem_acme.rs   present → cfg(has_oem_acme)   is set
//   src/oem_sample.rs present → ignored (always compiled, open-source)
//
// In lib.rs these are consumed as:
//   #[cfg(has_oem_mbds)]
//   pub mod oem_mbds;
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    let src_dir = std::path::Path::new("src");

    // Re-run if any file in src/ changes
    println!("cargo:rerun-if-changed=src/");

    // Declare all possible OEM cfg flags to the check-cfg lint.
    // Add new entries here when adding support for additional OEM profiles.
    println!("cargo::rustc-check-cfg=cfg(has_oem_mbds)");

    if let Ok(entries) = std::fs::read_dir(src_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Match oem_*.rs but skip oem_sample.rs (always available)
            if name_str.starts_with("oem_")
                && name_str.ends_with(".rs")
                && name_str != "oem_sample.rs"
            {
                // oem_mbds.rs → has_oem_mbds
                let cfg_name = name_str.trim_end_matches(".rs");
                let cfg_flag = format!("has_{cfg_name}");
                println!("cargo:rustc-cfg={cfg_flag}");
                eprintln!("  OEM profile detected: {name_str} → cfg({cfg_flag})");
            }
        }
    }
}
