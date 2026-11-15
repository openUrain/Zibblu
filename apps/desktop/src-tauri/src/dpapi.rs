use anyhow::{Result, ensure};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::Zeroize;

pub(crate) fn transform(bytes: &[u8], scope: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    fn blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: bytes.len().try_into()?,
            pbData: bytes.as_ptr().cast_mut(),
        })
    }
    let input = blob(bytes)?;
    let entropy = blob(scope)?;
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // DPAPI reads the input blobs and allocates the output with LocalAlloc.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                null(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    ensure!(
        success != 0 && !output.pbData.is_null(),
        "Windows credential protection failed"
    );
    let bytes = unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let result = bytes.to_vec();
    bytes.zeroize();
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(result)
}
