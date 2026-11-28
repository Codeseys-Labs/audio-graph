// Capability-blind raw core for the one macOS ACL operation the authority
// journal needs: replacing the extended ACL on a held descriptor with none.

#[cfg(target_os = "macos")]
use std::ffi::{c_int, c_void};
#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, BorrowedFd};

#[cfg(target_os = "macos")]
const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;
#[cfg(target_os = "macos")]
const ACL_FIRST_ENTRY: c_int = 0;
#[cfg(target_os = "macos")]
const ENOENT: i32 = 2;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_init(count: c_int) -> *mut c_void;
    fn acl_set_fd_np(fd: c_int, acl: *mut c_void, acl_type: c_int) -> c_int;
    fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_free(object: *mut c_void) -> c_int;
}

/// Sets an empty extended ACL on `fd` (what `chmod -N` does), then re-reads the
/// ACL from the same descriptor and proves it has no entry.
///
/// Afterwards only the owner and the mode bits decide access to the object, so
/// no inherited or explicit allow entry can grant another principal access.
#[cfg(target_os = "macos")]
pub(super) fn clear_extended_acl(fd: BorrowedFd<'_>) -> Result<(), ()> {
    let raw = fd.as_raw_fd();
    // SAFETY: acl_init returns an owned ACL or null; it is freed exactly once.
    let empty = unsafe { acl_init(0) };
    if empty.is_null() {
        return Err(());
    }
    // SAFETY: `raw` is a live descriptor and `empty` a valid ACL.
    let set = unsafe { acl_set_fd_np(raw, empty, ACL_TYPE_EXTENDED) };
    // SAFETY: `empty` came from acl_init and is not used again.
    unsafe { acl_free(empty) };
    if set != 0 {
        return Err(());
    }
    // SAFETY: `raw` is a live descriptor.
    let current = unsafe { acl_get_fd_np(raw, ACL_TYPE_EXTENDED) };
    if current.is_null() {
        // Null with ENOENT means the object carries no extended ACL at all.
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(ENOENT) => Ok(()),
            _ => Err(()),
        };
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: `current` is a valid ACL and `entry` a writable out-pointer.
    let has_entry = unsafe { acl_get_entry(current, ACL_FIRST_ENTRY, &mut entry) } == 0;
    // SAFETY: `current` came from acl_get_fd_np and is not used again.
    unsafe { acl_free(current) };
    if has_entry { Err(()) } else { Ok(()) }
}
