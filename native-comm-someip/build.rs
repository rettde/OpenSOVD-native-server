// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// build.rs — Compile vsomeip C++ wrapper when feature "vsomeip-ffi" is enabled
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    #[cfg(feature = "vsomeip-ffi")]
    {
        // Compile the C++ wrapper
        cc::Build::new()
            .cpp(true)
            .std("c++14")
            .file("ffi/vsomeip_wrapper.cpp")
            .include("ffi")
            // vsomeip3 headers are typically in /usr/include or /usr/local/include
            .include("/usr/include")
            .include("/usr/local/include")
            .warnings(true)
            .compile("vsomeip_wrapper");

        // Link against vsomeip3
        println!("cargo:rustc-link-lib=dylib=vsomeip3");
        println!("cargo:rustc-link-lib=dylib=stdc++");

        // Re-run if wrapper sources change
        println!("cargo:rerun-if-changed=ffi/vsomeip_wrapper.h");
        println!("cargo:rerun-if-changed=ffi/vsomeip_wrapper.cpp");

        // Search paths for libvsomeip3.so
        println!("cargo:rustc-link-search=native=/usr/lib");
        println!("cargo:rustc-link-search=native=/usr/local/lib");

        // Allow overriding vsomeip install prefix via env
        if let Ok(prefix) = std::env::var("VSOMEIP_PREFIX") {
            println!("cargo:rustc-link-search=native={prefix}/lib");
            println!("cargo:rustc-flags=-L {prefix}/lib");
        }
    }
}
