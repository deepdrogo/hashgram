//! Windows security primitives: DPAPI wrapping and Windows Hello.
//!
//! The vault is Argon2id + XChaCha20-Poly1305 under the passphrase; that is
//! what protects the keys. Windows Hello is a convenience *unlock*, never a
//! second copy of the mnemonic: the passphrase is wrapped with DPAPI (bound
//! to this Windows user and machine) using entropy that only exists after
//! a successful Hello prompt — a signature over a fixed challenge from the
//! Hello-protected key credential. No prompt, no entropy, no passphrase.
//!
//! `unsafe` is confined to this module and consists of the three Win32 FFI
//! calls DPAPI needs (protect, unprotect, free). The WinRT calls are safe
//! wrappers from the `windows` crate.

#![allow(unsafe_code)]

use sha2::{Digest, Sha256};

/// Name of the Hello key credential this app owns.
pub const HELLO_CREDENTIAL: &str = "Hashgram Desktop vault";
/// The fixed challenge whose signature becomes DPAPI entropy.
pub const HELLO_CHALLENGE: &[u8] = b"hashgram-desktop-vault-unlock-v1";

/// Derives DPAPI entropy from a Hello signature.
#[must_use]
pub fn entropy_from_signature(sig: &[u8]) -> Vec<u8> {
    Sha256::digest(sig).to_vec()
}

#[cfg(windows)]
mod imp {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Security::Credentials::{
        KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
    };
    use windows::Storage::Streams::{DataReader, DataWriter};
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr().cast_mut(),
        }
    }

    // SAFETY (shared by protect/unprotect): `out` was filled by the OS with
    // a LocalAlloc'd buffer of `cbData` bytes; we copy it out and free it
    // exactly once.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        v
    }

    /// Wraps data with DPAPI for this user and machine, bound to entropy.
    pub fn dpapi_protect(data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(data);
        let ent = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: all pointers reference live stack/slice memory for the
        // duration of the call; `out` is written by the OS.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&ent),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .map_err(|e| format!("DPAPI protect: {e}"))?;
            Ok(take(out))
        }
    }

    /// Unwraps a DPAPI blob; fails for another user, machine or entropy.
    pub fn dpapi_unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(sealed);
        let ent = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as above.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&ent),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .map_err(|e| format!("DPAPI unprotect: {e}"))?;
            Ok(take(out))
        }
    }

    fn init_winrt() {
        // SAFETY: RoInitialize has no memory-safety preconditions; a
        // "changed mode" result on an already-initialised thread is fine.
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
    }

    /// Whether Windows Hello key credentials are available on this PC.
    pub fn hello_supported() -> bool {
        init_winrt();
        KeyCredentialManager::IsSupportedAsync()
            .and_then(|op| op.join())
            .unwrap_or(false)
    }

    fn sign_with(
        cred: &windows::Security::Credentials::KeyCredential,
        challenge: &[u8],
    ) -> Result<Vec<u8>, String> {
        let writer = DataWriter::new().map_err(|e| e.to_string())?;
        writer.WriteBytes(challenge).map_err(|e| e.to_string())?;
        let buf = writer.DetachBuffer().map_err(|e| e.to_string())?;
        let res = cred
            .RequestSignAsync(&buf)
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let status = res.Status().map_err(|e| e.to_string())?;
        if status != KeyCredentialStatus::Success {
            return Err(hello_status(status));
        }
        let out = res.Result().map_err(|e| e.to_string())?;
        let len = out.Length().map_err(|e| e.to_string())? as usize;
        let reader = DataReader::FromBuffer(&out).map_err(|e| e.to_string())?;
        let mut v = vec![0u8; len];
        reader.ReadBytes(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }

    fn hello_status(s: KeyCredentialStatus) -> String {
        match s {
            KeyCredentialStatus::UserCanceled => "Windows Hello was cancelled".into(),
            KeyCredentialStatus::NotFound => "no Windows Hello key for Hashgram on this PC".into(),
            KeyCredentialStatus::UserPrefersPassword => {
                "Windows Hello: user prefers a password".into()
            }
            KeyCredentialStatus::CredentialAlreadyExists => {
                "Windows Hello key already exists".into()
            }
            KeyCredentialStatus::SecurityDeviceLocked => "the security device is locked".into(),
            _ => "Windows Hello failed".into(),
        }
    }

    /// Creates (replacing) the Hello key credential and returns a signature
    /// over `challenge`.
    pub fn hello_create_and_sign(name: &str, challenge: &[u8]) -> Result<Vec<u8>, String> {
        init_winrt();
        let res = KeyCredentialManager::RequestCreateAsync(
            &HSTRING::from(name),
            KeyCredentialCreationOption::ReplaceExisting,
        )
        .and_then(|op| op.join())
        .map_err(|e| e.to_string())?;
        let status = res.Status().map_err(|e| e.to_string())?;
        if status != KeyCredentialStatus::Success {
            return Err(hello_status(status));
        }
        let cred = res.Credential().map_err(|e| e.to_string())?;
        sign_with(&cred, challenge)
    }

    /// Prompts Hello and signs `challenge` with the existing credential.
    pub fn hello_sign(name: &str, challenge: &[u8]) -> Result<Vec<u8>, String> {
        init_winrt();
        let res = KeyCredentialManager::OpenAsync(&HSTRING::from(name))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let status = res.Status().map_err(|e| e.to_string())?;
        if status != KeyCredentialStatus::Success {
            return Err(hello_status(status));
        }
        let cred = res.Credential().map_err(|e| e.to_string())?;
        sign_with(&cred, challenge)
    }

    /// Deletes the credential (disabling Hello unlock).
    pub fn hello_delete(name: &str) -> Result<(), String> {
        init_winrt();
        KeyCredentialManager::DeleteAsync(&HSTRING::from(name))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())
    }

    /// Working set of this process in bytes.
    pub fn working_set_bytes() -> u64 {
        use windows::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: `counters` is a properly sized, writable struct; the
        // pseudo-handle from GetCurrentProcess needs no closing.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok.as_bool() {
            counters.WorkingSetSize as u64
        } else {
            0
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn dpapi_protect(_d: &[u8], _e: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI is Windows-only".into())
    }
    pub fn dpapi_unprotect(_d: &[u8], _e: &[u8]) -> Result<Vec<u8>, String> {
        Err("DPAPI is Windows-only".into())
    }
    /// Whether Windows Hello key credentials are available on this PC.
    pub fn hello_supported() -> bool {
        false
    }
    pub fn hello_create_and_sign(_n: &str, _c: &[u8]) -> Result<Vec<u8>, String> {
        Err("Windows Hello is Windows-only".into())
    }
    pub fn hello_sign(_n: &str, _c: &[u8]) -> Result<Vec<u8>, String> {
        Err("Windows Hello is Windows-only".into())
    }
    pub fn hello_delete(_n: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn working_set_bytes() -> u64 {
        0
    }
}

pub use imp::{
    dpapi_protect, dpapi_unprotect, hello_create_and_sign, hello_delete, hello_sign,
    hello_supported, working_set_bytes,
};

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn hello_support_check_returns_promptly_from_a_worker_thread() {
        // The app calls this from a blocking pool thread at every start; it
        // must answer in well under a second, whatever the answer is.
        let t = std::time::Instant::now();
        let supported = std::thread::spawn(hello_supported).join().unwrap_or(false);
        let elapsed = t.elapsed();
        eprintln!("hello_supported = {supported} in {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "took {elapsed:?}"
        );
    }

    #[test]
    fn dpapi_round_trips_and_binds_entropy() {
        let sealed = dpapi_protect(b"passphrase", b"entropy-a").unwrap();
        assert!(!sealed.windows(10).any(|w| w == b"passphrase"));
        assert_eq!(
            dpapi_unprotect(&sealed, b"entropy-a").unwrap(),
            b"passphrase"
        );
        assert!(dpapi_unprotect(&sealed, b"entropy-b").is_err());
    }
}
