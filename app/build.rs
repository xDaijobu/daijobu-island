//! Embeds the Per-Monitor-V2 DPI manifest into `daijobu.exe` through the MSVC linker.

fn main() {
    // Track the worktree at build-script compile time. A shared target directory
    // must not reuse a cached linker path into a removed sibling checkout.
    let dir = env!("CARGO_MANIFEST_DIR");
    embed_notification_payload();
    println!("cargo:rerun-if-changed={dir}/daijobu.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{dir}/daijobu.manifest");
    }
}

fn embed_notification_payload() {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=DAIJOBU_NOTIFICATION_MSIX");
    println!("cargo:rerun-if-env-changed=DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = if let Some(path) = env::var_os("DAIJOBU_NOTIFICATION_MSIX") {
        let path = PathBuf::from(path)
            .canonicalize()
            .expect("DAIJOBU_NOTIFICATION_MSIX must point to an existing signed sparse MSIX");
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path).expect("read identity MSIX");
        assert!(
            bytes.len() <= 16 * 1024 * 1024 && bytes.starts_with(b"PK"),
            "identity payload must be an MSIX archive no larger than 16 MiB"
        );
        assert!(
            bytes
                .windows(b"AppxSignature.p7x".len())
                .any(|w| w == b"AppxSignature.p7x"),
            "identity payload has no package signature; sign it at build time"
        );
        // This is an embedding sanity check, not signature verification. The
        // packaging script verifies with SignTool; Windows verifies on deployment.
        fs::write(output.join("notification-identity.msix"), bytes).unwrap();
        "pub const SIGNED_MSIX: Option<&[u8]> = Some(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/notification-identity.msix\")));"
    } else {
        assert!(
            env::var("DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD").as_deref() != Ok("1"),
            "distribution builds require DAIJOBU_NOTIFICATION_MSIX"
        );
        "pub const SIGNED_MSIX: Option<&[u8]> = None;"
    };
    fs::write(output.join("notification_payload.rs"), source).unwrap();
}
