//! Current-user, non-interactive DPAPI. No plaintext fallback is permitted.

pub(crate) trait Crypto: Send {
    fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>, String>;
    fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>, String>;
}

pub(crate) struct Dpapi;

#[cfg(windows)]
fn transform(input: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    const MAX_BLOB: usize = 100 * 1024 * 1024;
    if input.len() > MAX_BLOB {
        return Err("Clipboard encryption size limit".into());
    }
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: The input stays alive through this synchronous call. Output is
    // initialized; no prompts, machine scope, or description are requested.
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &source,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    let bytes = if result.is_ok()
        && output.cbData as usize <= MAX_BLOB
        && (output.cbData == 0 || !output.pbData.is_null())
    {
        if output.cbData == 0 {
            Ok(Vec::new())
        } else {
            // SAFETY: Successful DPAPI calls allocate cbData readable bytes.
            Ok(
                unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }
                    .to_vec(),
            )
        }
    } else {
        Err(if encrypt {
            "Clipboard encryption failed"
        } else {
            "Clipboard decryption failed"
        }
        .into())
    };
    if !output.pbData.is_null() {
        // SAFETY: DPAPI allocated this buffer and requires LocalFree exactly once.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        }
    }
    bytes
}

#[cfg(not(windows))]
fn transform(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("Clipboard encryption unavailable".into())
}

impl Crypto for Dpapi {
    fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        transform(plain, true)
    }
    fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>, String> {
        transform(encrypted, false)
    }
}

/// Reversible test fixture; never selected by Store::new.
#[cfg(test)]
pub(crate) struct FakeCrypto;
#[cfg(test)]
impl Crypto for FakeCrypto {
    fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        let mut result = b"test-only-v1".to_vec();
        result.extend(plain.iter().map(|b| b ^ 0xa5));
        Ok(result)
    }
    fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>, String> {
        let data = encrypted
            .strip_prefix(b"test-only-v1")
            .ok_or("Clipboard decryption failed")?;
        Ok(data.iter().map(|b| b ^ 0xa5).collect())
    }
}
