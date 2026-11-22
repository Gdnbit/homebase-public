//! Windows PowerShell ConvertFrom-SecureString (current-user DPAPI, no AES key).
//! No subprocess, stdout, environment injection, serialization or plaintext file.
use super::sops::SecureString;
use std::{fs, io::Read, path::Path};
use zeroize::{Zeroize, Zeroizing};

pub fn read_current_user(path: &Path) -> Result<SecureString, &'static str> {
    let mut file = fs::File::open(path).map_err(|_| "credential_unreadable")?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.by_ref()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| "credential_unreadable")?;
    if bytes.len() > 65_536 {
        return Err("credential_too_large");
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "credential_format_invalid")?
        .trim_start_matches('\u{feff}')
        .trim();
    if text.is_empty() || text.len() % 2 != 0 || !text.starts_with("01000000") {
        return Err("credential_format_invalid");
    }
    let mut blob = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for pair in text.as_bytes().chunks_exact(2) {
        let hex = |c: u8| {
            (c as char)
                .to_digit(16)
                .map(|v| v as u8)
                .ok_or("credential_format_invalid")
        };
        blob.push(hex(pair[0])? * 16 + hex(pair[1])?);
    }
    unprotect(&mut blob)
}

#[cfg(windows)]
fn unprotect(bytes: &mut [u8]) -> Result<SecureString, &'static str> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI owns the allocation; always wipe it before LocalFree, including errors.
    struct PlainBlob(CRYPT_INTEGER_BLOB);
    impl Drop for PlainBlob {
        fn drop(&mut self) {
            unsafe {
                if !self.0.pbData.is_null() {
                    std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                    LocalFree(self.0.pbData.cast());
                }
            }
        }
    }
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    let plain = PlainBlob(output);
    if ok == 0 {
        return Err("current_user_dpapi_failed");
    }
    if plain.0.pbData.is_null()
        || plain.0.cbData == 0
        || plain.0.cbData > 32_768
        || plain.0.cbData % 2 != 0
    {
        return Err("credential_plaintext_invalid");
    }
    let raw = unsafe { std::slice::from_raw_parts(plain.0.pbData, plain.0.cbData as usize) };
    let chars = Zeroizing::new(
        raw.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    );
    let mut text = Zeroizing::new(String::with_capacity(raw.len()));
    for character in char::decode_utf16(chars.iter().copied()) {
        text.push(character.map_err(|_| "credential_plaintext_invalid")?);
    }
    let secret = SecureString::new(std::mem::take(&mut *text));
    if secret.is_empty()
        || secret.as_str().trim().is_empty()
        || secret.as_str().contains(['\0', '\r', '\n'])
    {
        return Err("credential_plaintext_invalid");
    }
    Ok(secret)
}

#[cfg(not(windows))]
fn unprotect(_: &mut [u8]) -> Result<SecureString, &'static str> {
    Err("windows_current_user_required")
}

#[cfg(all(test, windows))]
pub(crate) fn write_synthetic(path: &Path, value: &str) {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
    };
    let mut plain = Zeroizing::new(
        value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    assert_ne!(
        unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        },
        0
    );
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let cipher = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    unsafe {
        LocalFree(output.pbData.cast());
    }
    fs::write(path, cipher).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dpapi_bridge_invalid_input_never_becomes_a_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.dpapi");
        for bad in ["", "synthetic-sensitive", "01000000xx", "0100000000"] {
            fs::write(&path, bad).unwrap();
            let error = read_current_user(&path).unwrap_err();
            assert!(!error.contains("synthetic-sensitive"));
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_bridge_roundtrip_is_memory_only_and_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.dpapi");
        write_synthetic(&path, "synthetic-test-凭据");
        let before = fs::read(&path).unwrap();
        let value = read_current_user(&path).unwrap();
        assert_eq!(value.as_str(), "synthetic-test-凭据");
        assert!(!format!("{value:?}").contains("synthetic-test"));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
