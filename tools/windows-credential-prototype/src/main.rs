//! Windows Credential Manager feasibility spike kept outside application crates.
//!
//! It writes only fixed non-secret test bytes to a fresh session-persisted
//! Generic Credential, verifies the round trip, and removes it before exit.

#[cfg(windows)]
use windows::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
#[cfg(windows)]
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_SESSION, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
#[cfg(windows)]
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
const TEST_BLOB: [u8; 32] = [
    0x53, 0x6f, 0x6e, 0x69, 0x4d, 0x65, 0x73, 0x68, 0x20, 0x43, 0x72, 0x65, 0x64, 0x65, 0x6e, 0x74,
    0x69, 0x61, 0x6c, 0x20, 0x53, 0x70, 0x69, 0x6b, 0x65, 0x20, 0x76, 0x31, 0x00, 0x00, 0x00, 0x01,
];

#[cfg(windows)]
const TARGET_NONCE_BYTES: usize = 16;
#[cfg(windows)]
const TARGET_PREFIX: &str = "SoniMesh/credential-spike/";
#[cfg(windows)]
const HEX: &[u8; 16] = b"0123456789abcdef";

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = target_name()?;
    let target_wide = nul_terminated_utf16(&target);
    let mut test_blob = TEST_BLOB;

    // A stale entry only arises from an interrupted prior run with the same target.
    let _ = unsafe { CredDeleteW(PCWSTR(target_wide.as_ptr()), CRED_TYPE_GENERIC, None) };

    let mut cleanup = CleanupGuard::new(&target_wide);
    write_credential(&target_wide, &mut test_blob)?;
    cleanup.mark_written();
    println!("write: ok");

    let readback = read_credential(&target_wide)?;
    if readback != TEST_BLOB {
        return Err("credential readback differed from the fixed test blob".into());
    }
    println!("read: ok ({} bytes)", readback.len());

    cleanup.delete()?;
    println!("delete: ok");

    let category = read_after_delete_category(&target_wide)?;
    println!("read_after_delete: {category}");
    if category != "not_found" {
        return Err("credential remained readable after deletion".into());
    }
    Ok(())
}

#[cfg(windows)]
fn target_name() -> Result<String, windows::core::Error> {
    let mut nonce = [0_u8; TARGET_NONCE_BYTES];
    // A null algorithm handle with this flag requests the Windows system-preferred RNG.
    let status = unsafe { BCryptGenRandom(None, &mut nonce, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if !status.is_ok() {
        return Err(status.into());
    }
    Ok(target_name_from_nonce(&nonce))
}

#[cfg(windows)]
fn target_name_from_nonce(nonce: &[u8; TARGET_NONCE_BYTES]) -> String {
    let mut name = String::with_capacity(TARGET_PREFIX.len() + nonce.len() * 2);
    name.push_str(TARGET_PREFIX);
    for &byte in nonce {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    name
}

#[cfg(windows)]
fn nul_terminated_utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn write_credential(target: &[u16], blob: &mut [u8]) -> Result<(), Box<dyn std::error::Error>> {
    let blob_size = u32::try_from(blob.len())?;
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(target.as_ptr().cast_mut()),
        CredentialBlobSize: blob_size,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_SESSION,
        ..Default::default()
    };
    // The target and blob remain valid for the synchronous WinCred call.
    unsafe { CredWriteW(&credential, 0) }?;
    Ok(())
}

#[cfg(windows)]
fn read_credential(target: &[u16]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let allocation = CredentialAllocation::read(target)?;
    let credential = allocation.credential()?;
    if credential.CredentialBlob.is_null() {
        return Err(std::io::Error::other("Credential Manager returned a null blob").into());
    }
    let byte_count = usize::try_from(credential.CredentialBlobSize)?;
    // CredReadW owns a valid blob for CredentialBlobSize bytes until CredFree.
    let bytes =
        unsafe { std::slice::from_raw_parts(credential.CredentialBlob.cast_const(), byte_count) }
            .to_vec();
    Ok(bytes)
}

#[cfg(windows)]
fn read_after_delete_category(target: &[u16]) -> Result<&'static str, Box<dyn std::error::Error>> {
    match CredentialAllocation::read(target) {
        Ok(_allocation) => Ok("still_present"),
        Err(error) if is_not_found(&error) => Ok("not_found"),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
struct CredentialAllocation {
    raw: *mut CREDENTIALW,
}

#[cfg(windows)]
impl CredentialAllocation {
    fn read(target: &[u16]) -> Result<Self, std::io::Error> {
        let mut raw = std::ptr::null_mut();
        match unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut raw) } {
            Ok(()) => Ok(Self { raw }),
            Err(_) => Err(last_error()),
        }
    }

    fn credential(&self) -> Result<&CREDENTIALW, std::io::Error> {
        unsafe { self.raw.as_ref() }
            .ok_or_else(|| std::io::Error::other("CredReadW succeeded with a null allocation"))
    }
}

#[cfg(windows)]
fn last_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(error_code(unsafe { GetLastError() }))
}

#[cfg(windows)]
fn error_code(error: windows::Win32::Foundation::WIN32_ERROR) -> i32 {
    i32::try_from(error.0).unwrap_or(i32::MAX)
}

#[cfg(windows)]
fn is_not_found(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(error_code(ERROR_NOT_FOUND))
}

#[cfg(windows)]
impl Drop for CredentialAllocation {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // CredReadW documented this allocation as caller-owned through CredFree.
            unsafe { CredFree(self.raw.cast()) };
        }
    }
}

#[cfg(windows)]
struct CleanupGuard<'a> {
    target: &'a [u16],
    written: bool,
}

#[cfg(windows)]
impl<'a> CleanupGuard<'a> {
    const fn new(target: &'a [u16]) -> Self {
        Self {
            target,
            written: false,
        }
    }

    fn mark_written(&mut self) {
        self.written = true;
    }

    fn delete(&mut self) -> windows::core::Result<()> {
        if self.written {
            unsafe { CredDeleteW(PCWSTR(self.target.as_ptr()), CRED_TYPE_GENERIC, None) }?;
            self.written = false;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for CleanupGuard<'_> {
    fn drop(&mut self) {
        if self.written {
            let _ = unsafe { CredDeleteW(PCWSTR(self.target.as_ptr()), CRED_TYPE_GENERIC, None) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn opaque_target_name_encodes_a_fixed_nonce_without_test_bytes() {
        let first = target_name_from_nonce(&[0; TARGET_NONCE_BYTES]);
        let second = target_name_from_nonce(&[0xff; TARGET_NONCE_BYTES]);

        assert_eq!(
            first,
            "SoniMesh/credential-spike/00000000000000000000000000000000"
        );
        assert_eq!(
            second,
            "SoniMesh/credential-spike/ffffffffffffffffffffffffffffffff"
        );
        assert!(!first.contains("Credential Spike"));
    }

    #[test]
    fn system_rng_generates_distinct_opaque_target_names() {
        let first = target_name().unwrap();
        let second = target_name().unwrap();

        assert_ne!(first, second);
        assert!(first.starts_with(TARGET_PREFIX));
        assert_eq!(first.len(), TARGET_PREFIX.len() + TARGET_NONCE_BYTES * 2);
        assert!(!first.contains("Credential Spike"));
    }

    #[test]
    fn not_found_mapping_uses_the_windows_error_code() {
        let missing = std::io::Error::from_raw_os_error(error_code(ERROR_NOT_FOUND));
        assert!(is_not_found(&missing));
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("windows-credential-prototype requires Windows");
}
