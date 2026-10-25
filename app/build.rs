//! Embeds the Per-Monitor-V2 DPI manifest into `daijobu.exe` through the MSVC linker.

fn main() {
    println!("cargo:rerun-if-changed=daijobu.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/daijobu.manifest");
    }
}
