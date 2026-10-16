//! TxF (Transactional NTFS) bindings for the atomic `.env` edit. A
//! transacted writer locks both data and namespace identity — including
//! against non-transacted rename-and-save editors — for the lifetime of the
//! transaction, which a plain open/read/rename cannot provide. TxF
//! unavailability is a stable public failure with no unsafe fallback.

use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use windows::Win32::Foundation::HANDLE;
use windows::core::PCWSTR;

// ktmw32.dll transaction API, declared locally with an explicit link so the
// dependency stays auditable and version-independent.
#[link(name = "ktmw32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateTransaction(
        lptransactionattributes: *const core::ffi::c_void,
        guid: *const core::ffi::c_void,
        options: u32,
        reserved: u32,
        stacksize: u32,
        timeout: u32,
        description: PCWSTR,
    ) -> HANDLE;
    fn CommitTransaction(transaction: HANDLE) -> i32;
    fn RollbackTransaction(transaction: HANDLE) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileTransactedW(
        lpfilename: PCWSTR,
        dwdesiredaccess: u32,
        dwsharemode: u32,
        lpsecurityattributes: *const core::ffi::c_void,
        dwcreationdisposition: u32,
        dwflagsandattributes: u32,
        htemplatefile: HANDLE,
        htransaction: HANDLE,
        pusminiversion: *const core::ffi::c_void,
        lpextendedparameter: *const core::ffi::c_void,
    ) -> HANDLE;
    fn DeleteFileTransactedW(lpfilename: PCWSTR, htransaction: HANDLE) -> i32;
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_FUNCTION: u32 = 1;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE: u32 = 6803;
const ERROR_EFS_NOT_ALLOWED_IN_TRANSACTION: u32 = 6012;
const ERROR_ALREADY_EXISTS: u32 = 183;

pub struct Transaction {
    handle: OwnedHandle,
    committed: bool,
}

impl Transaction {
    pub fn new() -> Result<Transaction> {
        // SAFETY: creation with defaults and a null description; the returned
        // handle is owned by this object and rolled back in Drop unless
        // commit succeeded.
        let handle = unsafe {
            CreateTransaction(
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                0,
                5000,
                PCWSTR::null(),
            )
        };
        let owned = OwnedHandle::new(handle).ok_or_else(|| {
            Error::new(
                "BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE",
                "Windows file transactions are unavailable; the proxy file was not changed.",
            )
        })?;
        Ok(Transaction {
            handle: owned,
            committed: false,
        })
    }

    pub fn raw(&self) -> HANDLE {
        self.handle.raw()
    }

    pub fn commit(mut self) -> Result<()> {
        // SAFETY: owned, valid transaction handle; consumed exactly once.
        if unsafe { CommitTransaction(self.handle.raw()) } == 0 {
            return Err(Error::new(
                "BACKEND_PROXY_ENV_IO",
                "The proxy file transaction could not commit; no partial edit was published.",
            ));
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.committed {
            // SAFETY: owned handle; rollback of an already-committed or
            // doomed transaction is a harmless no-op returning failure.
            unsafe { RollbackTransaction(self.handle.raw()) };
        }
    }
}

#[derive(Debug)]
pub enum TransactedOpen {
    File(OwnedHandle),
    Conflict,
    Unavailable,
}

/// Opens the target inside the transaction with full share flags. Returns a
/// typed classification for the not-found/exists family (conflict), the
/// TxF-unsupported family (unavailable) and generic I/O failure (Err).
pub fn create_file_transacted(
    path: &std::path::Path,
    exists: bool,
    transaction: &Transaction,
) -> Result<TransactedOpen> {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    const GENERIC_READ_WRITE_DELETE: u32 = 0x8000_0000 | 0x4000_0000 | 0x0001_0000;
    const FILE_SHARE_ALL: u32 = 0x7;
    const OPEN_EXISTING: u32 = 3;
    const CREATE_NEW: u32 = 1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    // SAFETY: null-terminated path; the handle (when valid) is owned.
    let handle = unsafe {
        CreateFileTransactedW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ_WRITE_DELETE,
            FILE_SHARE_ALL,
            std::ptr::null(),
            if exists { OPEN_EXISTING } else { CREATE_NEW },
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
            transaction.raw(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if let Some(owned) = OwnedHandle::new(handle) {
        return Ok(TransactedOpen::File(owned));
    }
    let error = unsafe { windows::Win32::Foundation::GetLastError() }.0;
    match error {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS => {
            Ok(TransactedOpen::Conflict)
        }
        ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE
        | ERROR_NOT_SUPPORTED
        | ERROR_INVALID_FUNCTION
        | ERROR_EFS_NOT_ALLOWED_IN_TRANSACTION => Ok(TransactedOpen::Unavailable),
        ERROR_ACCESS_DENIED | ERROR_INVALID_PARAMETER => Err(Error::new(
            "BACKEND_PROXY_ENV_IO",
            "Cannot lock the proxy file for an atomic transaction.",
        )),
        _ => Err(Error::new(
            "BACKEND_PROXY_ENV_IO",
            "Cannot lock the proxy file for an atomic transaction.",
        )),
    }
}

/// Transactional delete; mirrors `DeleteFileTransactedW` semantics.
pub fn delete_file_transacted(path: &std::path::Path, transaction: &Transaction) -> Result<()> {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: null-terminated path; owned transaction handle.
    if unsafe { DeleteFileTransactedW(PCWSTR(wide.as_ptr()), transaction.raw()) } == 0 {
        return Err(Error::new(
            "BACKEND_PROXY_ENV_IO",
            "Cannot remove the managed proxy file transactionally.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_commits_and_rolls_back_files() {
        let directory = std::env::temp_dir().join(format!("cpg-txf-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("txf-env");
        std::fs::write(&path, b"kept").expect("write");

        // Rollback path: write inside the transaction, drop without commit.
        {
            let transaction = Transaction::new().expect("TxF available on the system drive");
            let file = match create_file_transacted(&path, true, &transaction).expect("open") {
                TransactedOpen::File(file) => file,
                other => panic!("unexpected open result: {other:?}"),
            };
            crate::fsio::write_raw(file.raw(), b"staged")
                .then_some(())
                .expect("staged write");
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"kept",
            "rollback restored nothing"
        );

        // Commit path.
        {
            let transaction = Transaction::new().expect("TxF available");
            let file = match create_file_transacted(&path, true, &transaction).expect("open") {
                TransactedOpen::File(file) => file,
                other => panic!("unexpected open result: {other:?}"),
            };
            crate::fsio::write_raw(file.raw(), b"committed")
                .then_some(())
                .expect("write");
            drop(file);
            transaction.commit().expect("commit");
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"committed");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn missing_file_classifies_as_conflict() {
        let transaction = Transaction::new().expect("TxF available");
        let missing = std::env::temp_dir().join("cpg-txf-definitely-missing-file");
        match create_file_transacted(&missing, true, &transaction).expect("open result") {
            TransactedOpen::Conflict => {}
            other => panic!("expected conflict, got {other:?}"),
        }
    }
}
