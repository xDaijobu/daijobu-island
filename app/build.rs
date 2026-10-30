//! Embeds the Per-Monitor-V2 DPI manifest into `daijobu.exe` through the MSVC linker.

fn main() {
    // Track the worktree at build-script compile time. A shared target directory
    // must not reuse a cached linker path into a removed sibling checkout.
    let dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-changed={dir}/daijobu.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/daijobu.manifest");
    }
}
