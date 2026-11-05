// Included by the build scripts of the programs Trek ships on Windows (trek, trek-mcp, trek-update).

/// Link the Visual C++ runtime into the program, so it starts on a Windows that never had the
/// redistributable installed; the Universal CRT stays the system's, which every supported Windows
/// has. Done here, not with `+crt-static` in `.cargo/config.toml`: CI and the release set
/// `RUSTFLAGS`, which makes Cargo ignore a config file's flags, and the release would ship without
/// it. `release.ps1` and `tests/hardening.rs` check that it took.
fn static_vcruntime() {
    println!("cargo:rerun-if-changed=../../script/static_vcruntime.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    for lib in [
        "libvcruntimed.lib",
        "vcruntime.lib",
        "vcruntimed.lib",
        "libcmtd.lib",
        "msvcrt.lib",
        "msvcrtd.lib",
        "libucrt.lib",
        "libucrtd.lib",
    ] {
        println!("cargo:rustc-link-arg-bins=/NODEFAULTLIB:{lib}");
    }
    for lib in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg-bins=/DEFAULTLIB:{lib}");
    }
}
