use anyhow::{Result, ensure};
use std::ffi::{CString, c_char};
use zeroize::Zeroizing;

unsafe extern "C" {
    fn zibblu_keychain_write(account: *const c_char, data: *const u8, length: usize) -> bool;
    fn zibblu_keychain_read(account: *const c_char, buffer: *mut u8, capacity: usize) -> isize;
    fn zibblu_keychain_delete(account: *const c_char) -> bool;
}

pub fn store_secret(account: &str, bytes: &[u8]) -> Result<()> {
    let account = CString::new(account)?;
    ensure!(
        unsafe { zibblu_keychain_write(account.as_ptr(), bytes.as_ptr(), bytes.len()) },
        "Could not write to the iOS Keychain"
    );
    Ok(())
}

pub fn read_secret(account: &str, limit: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let account = CString::new(account)?;
    let mut bytes = Zeroizing::new(vec![0; limit]);
    let length = unsafe { zibblu_keychain_read(account.as_ptr(), bytes.as_mut_ptr(), limit) };
    if length == -1 {
        return Ok(None);
    }
    ensure!(
        length >= 0 && length as usize <= limit,
        "Could not read the iOS Keychain"
    );
    bytes.truncate(length as usize);
    Ok(Some(bytes))
}

pub fn delete_secret(account: &str) -> Result<()> {
    let account = CString::new(account)?;
    ensure!(
        unsafe { zibblu_keychain_delete(account.as_ptr()) },
        "Could not clear the iOS Keychain"
    );
    Ok(())
}
