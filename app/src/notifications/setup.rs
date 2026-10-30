//! Immutable embedded inputs, extracted only after an explicit UI setup action.
use dj_core::notification_setup::Failure;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/notification_payload.rs"));
const MANIFEST: &[u8] = include_bytes!("../../../packaging/notifications/AppxManifest.xml");
const ASSETS: &[(&str, &[u8])] = &[
    (
        "StoreLogo.png",
        include_bytes!("../../../packaging/notifications/Assets/StoreLogo.png"),
    ),
    (
        "Square44x44Logo.png",
        include_bytes!("../../../packaging/notifications/Assets/Square44x44Logo.png"),
    ),
    (
        "Square150x150Logo.png",
        include_bytes!("../../../packaging/notifications/Assets/Square150x150Logo.png"),
    ),
];

pub fn prepare(root: &Path, exe: &Path) -> Result<(PathBuf, PathBuf, bool), Failure> {
    prepare_with(
        root,
        exe,
        SIGNED_MSIX,
        dj_win::notification_setup::developer_mode(),
    )
}

fn prepare_with(
    root: &Path,
    exe: &Path,
    signed: Option<&[u8]>,
    developer_mode: bool,
) -> Result<(PathBuf, PathBuf, bool), Failure> {
    let fail = |message: String| Failure {
        message,
        retryable: true,
    };
    if signed.is_none() && !developer_mode {
        return Err(fail("This build has no signed setup payload and Windows Developer Mode is off. Use a signed distribution build. For local development, you can enable Developer Mode yourself in Windows Settings, then retry. No Windows SDK is needed.".into()));
    }
    let exe = exe
        .canonicalize()
        .map_err(|e| fail(format!("Cannot locate this executable: {e}")))?;
    if exe
        .file_name()
        .is_none_or(|name| !name.eq_ignore_ascii_case("daijobu.exe"))
    {
        return Err(fail(
            "Keep the executable named daijobu.exe for Windows notification setup.".into(),
        ));
    }
    let external = exe
        .parent()
        .ok_or_else(|| fail("Cannot determine the app folder".into()))?
        .to_path_buf();
    // Separate stable directories for each external installation. Loose manifest
    // registration keeps referencing this directory, so never use a TempDir.
    let hash = external
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ *b as u64).wrapping_mul(0x100000001b3)
        });
    let payload = root.join(format!("notification-identity/0.3.1/{hash:016x}"));
    std::fs::create_dir_all(&payload)
        .map_err(|e| fail(format!("Cannot create setup payload folder: {e}")))?;
    for dir in [payload.join("Assets"), external.join("Assets")] {
        std::fs::create_dir_all(&dir).map_err(|e| fail(format!("Cannot create notification assets in {}: {e}. Move the app to a writable installation folder and retry.", dir.display())))?;
        for (name, bytes) in ASSETS {
            write_asset(&dir.join(name), bytes)?;
        }
    }
    let path = if let Some(bytes) = signed {
        let path = payload.join("DaijobuIsland.Notifications.msix");
        std::fs::write(&path, bytes)
            .map_err(|e| fail(format!("Cannot extract signed setup payload: {e}")))?;
        path
    } else {
        let path = payload.join("AppxManifest.xml");
        std::fs::write(&path, MANIFEST)
            .map_err(|e| fail(format!("Cannot extract development manifest: {e}")))?;
        path
    };
    Ok((path, external, signed.is_some()))
}

fn write_asset(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    use std::io::{Read, Write};
    let error = |message| Failure {
        message,
        retryable: true,
    };
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(bytes).map_err(|e| {
            error(format!(
                "Cannot write notification logo {}: {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let matches = (|| -> std::io::Result<bool> {
                let file = std::fs::File::open(path)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
                    return Ok(false);
                }
                let mut existing = Vec::with_capacity(bytes.len());
                file.take(bytes.len() as u64 + 1)
                    .read_to_end(&mut existing)?;
                Ok(existing == bytes)
            })()
            .unwrap_or(false);
            if matches {
                Ok(())
            } else {
                Err(error(format!("Notification logo {} conflicts with another file. Move the app into its own folder and retry.", path.display())))
            }
        }
        Err(e) => Err(error(format!(
            "Cannot extract notification logo {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn development_payload_is_stable_complete_and_handles_hostile_paths() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("app's #1 % 字");
        std::fs::create_dir(&external).unwrap();
        let exe = external.join("daijobu.exe");
        std::fs::write(&exe, b"fixture, never executed").unwrap();
        let root = temp.path().join("data");
        let first = prepare_with(&root, &exe, None, true).unwrap();
        assert!(!first.2);
        assert_eq!(std::fs::read(&first.0).unwrap(), MANIFEST);
        for (name, bytes) in ASSETS {
            assert_eq!(
                std::fs::read(first.0.parent().unwrap().join("Assets").join(name)).unwrap(),
                *bytes
            );
            assert_eq!(
                std::fs::read(first.1.join("Assets").join(name)).unwrap(),
                *bytes
            );
        }
        assert_eq!(prepare_with(&root, &exe, None, true).unwrap(), first);
    }
    #[test]
    fn unavailable_build_never_writes_and_signed_payload_never_falls_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let exe = temp.path().join("daijobu.exe");
        std::fs::write(&exe, b"fixture").unwrap();
        assert!(prepare_with(&root, &exe, None, false)
            .unwrap_err()
            .message
            .contains("Developer Mode is off"));
        assert!(!root.exists());
        let signed = b"signed fixture, never deployed";
        let (path, _, is_signed) = prepare_with(&root, &exe, Some(signed), true).unwrap();
        assert!(is_signed);
        assert_eq!(std::fs::read(path).unwrap(), signed);
        assert!(!root.join("AppxManifest.xml").exists());
    }
    #[test]
    fn foreign_assets_and_renamed_executables_are_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let exe = temp.path().join("renamed.exe");
        std::fs::write(&exe, b"fixture").unwrap();
        assert!(prepare_with(&root, &exe, None, true).is_err());
        let exe = temp.path().join("daijobu.exe");
        std::fs::write(&exe, b"fixture").unwrap();
        std::fs::create_dir(temp.path().join("Assets")).unwrap();
        let asset = temp.path().join("Assets/StoreLogo.png");
        std::fs::write(&asset, b"other app").unwrap();
        assert!(prepare_with(&root, &exe, None, true).is_err());
        assert_eq!(std::fs::read(asset).unwrap(), b"other app");
    }
    #[test]
    fn embedded_identity_matches_executable_manifest() {
        let package = std::str::from_utf8(MANIFEST).unwrap();
        let exe = include_str!("../../daijobu.manifest");
        for value in [
            "DaijobuIsland.Notifications",
            "CN=Daijobu Island",
            "Daijobu",
        ] {
            assert!(package.contains(value) && exe.contains(value));
        }
        assert!(package.contains("userNotificationListener"));
    }
}
