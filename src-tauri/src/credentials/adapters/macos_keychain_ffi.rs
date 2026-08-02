// All Security.framework `unsafe` and native ownership live in this capability-blind module.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::credentials::adapters) struct NativeStatus(i32);

impl NativeStatus {
    pub(in crate::credentials::adapters) const SUCCESS: Self = Self(0);
    pub(in crate::credentials::adapters) const INTERNAL_COMPONENT: Self = Self(-2070);
    pub(in crate::credentials::adapters) const UNIMPLEMENTED: Self = Self(-4);
    pub(in crate::credentials::adapters) const WRITE_PERMISSION: Self = Self(-61);
    pub(in crate::credentials::adapters) const USER_CANCELLED: Self = Self(-128);
    pub(in crate::credentials::adapters) const MISSING_ENTITLEMENT: Self = Self(-34018);
    pub(in crate::credentials::adapters) const RESTRICTED_API: Self = Self(-34020);
    pub(in crate::credentials::adapters) const NOT_AVAILABLE: Self = Self(-25291);
    pub(in crate::credentials::adapters) const READ_ONLY: Self = Self(-25292);
    pub(in crate::credentials::adapters) const AUTH_FAILED: Self = Self(-25293);
    pub(in crate::credentials::adapters) const NO_SUCH_KEYCHAIN: Self = Self(-25294);
    pub(in crate::credentials::adapters) const INVALID_KEYCHAIN: Self = Self(-25295);
    pub(in crate::credentials::adapters) const DUPLICATE_ITEM: Self = Self(-25299);
    pub(in crate::credentials::adapters) const ITEM_NOT_FOUND: Self = Self(-25300);
    pub(in crate::credentials::adapters) const DATA_TOO_LARGE: Self = Self(-25302);
    pub(in crate::credentials::adapters) const NO_DEFAULT_KEYCHAIN: Self = Self(-25307);
    pub(in crate::credentials::adapters) const INTERACTION_NOT_ALLOWED: Self = Self(-25308);
    pub(in crate::credentials::adapters) const READ_ONLY_ATTRIBUTE: Self = Self(-25309);
    pub(in crate::credentials::adapters) const WRONG_SECURITY_VERSION: Self = Self(-25310);
    pub(in crate::credentials::adapters) const NO_STORAGE_MODULE: Self = Self(-25312);
    pub(in crate::credentials::adapters) const INTERACTION_REQUIRED: Self = Self(-25315);
    pub(in crate::credentials::adapters) const DATA_NOT_MODIFIABLE: Self = Self(-25317);
    pub(in crate::credentials::adapters) const IN_DARK_WAKE: Self = Self(-25320);
    pub(in crate::credentials::adapters) const DECODE: Self = Self(-26275);
    pub(in crate::credentials::adapters) const SERVICE_NOT_AVAILABLE: Self = Self(-67585);

    #[cfg(test)]
    pub(in crate::credentials::adapters) const fn for_test(value: i32) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::credentials::adapters) struct KeychainStatus {
    pub(in crate::credentials::adapters) unlocked: bool,
    pub(in crate::credentials::adapters) readable: bool,
    pub(in crate::credentials::adapters) writable: bool,
}

#[cfg(target_os = "macos")]
use core_foundation::base::{Boolean, OSStatus, TCFType};
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain::SecKeychain;
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain_item::SecKeychainItem;
#[cfg(target_os = "macos")]
use security_framework_sys::base::{SecKeychainItemRef, SecKeychainRef};
#[cfg(target_os = "macos")]
use security_framework_sys::keychain::{
    SecKeychainAddGenericPassword, SecKeychainCopyDomainDefault, SecKeychainFindGenericPassword,
    SecKeychainGetUserInteractionAllowed, SecKeychainSetUserInteractionAllowed, SecKeychainUnlock,
    SecPreferencesDomain,
};
#[cfg(target_os = "macos")]
use security_framework_sys::keychain_item::{
    SecKeychainItemDelete, SecKeychainItemFreeContent, SecKeychainItemModifyAttributesAndData,
};
#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::ptr;
#[cfg(target_os = "macos")]
use zeroize::{Zeroize, Zeroizing};

#[cfg(target_os = "macos")]
const UNLOCKED_STATUS_BIT: u32 = 1;
#[cfg(target_os = "macos")]
const READABLE_STATUS_BIT: u32 = 2;
#[cfg(target_os = "macos")]
const WRITABLE_STATUS_BIT: u32 = 4;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn SecKeychainGetStatus(keychain: SecKeychainRef, keychain_status: *mut u32) -> OSStatus;
}

#[cfg(target_os = "macos")]
pub(in crate::credentials::adapters) struct SecurityFrameworkCore;

#[cfg(target_os = "macos")]
pub(in crate::credentials::adapters) struct UserKeychain(SecKeychain);

#[cfg(target_os = "macos")]
pub(in crate::credentials::adapters) struct KeychainItem(SecKeychainItem);

#[cfg(target_os = "macos")]
impl NativeStatus {
    fn from_raw(status: OSStatus) -> Self {
        Self(status)
    }

    fn checked(status: OSStatus) -> Result<(), Self> {
        if status == Self::SUCCESS.0 {
            Ok(())
        } else {
            Err(Self::from_raw(status))
        }
    }
}

#[cfg(target_os = "macos")]
struct NativePasswordBuffer {
    data: *mut c_void,
    length: usize,
    armed: bool,
}

#[cfg(target_os = "macos")]
impl NativePasswordBuffer {
    fn new(data: *mut c_void, length: u32) -> Self {
        Self {
            data,
            length: length as usize,
            armed: true,
        }
    }

    fn copy_and_release(mut self) -> Result<Zeroizing<Vec<u8>>, NativeStatus> {
        if self.length > 0 && self.data.is_null() {
            self.armed = false;
            return Err(NativeStatus::INTERNAL_COMPONENT);
        }
        let secret = if self.length == 0 {
            Zeroizing::new(Vec::new())
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(self.data.cast::<u8>(), self.length) };
            Zeroizing::new(bytes.to_vec())
        };
        self.release_checked()?;
        Ok(secret)
    }

    fn release_checked(&mut self) -> Result<(), NativeStatus> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        if self.data.is_null() {
            return if self.length == 0 {
                Ok(())
            } else {
                Err(NativeStatus::INTERNAL_COMPONENT)
            };
        }

        if self.length > 0 {
            let native_secret =
                unsafe { std::slice::from_raw_parts_mut(self.data.cast::<u8>(), self.length) };
            native_secret.zeroize();
        }
        let status = unsafe { SecKeychainItemFreeContent(ptr::null_mut(), self.data) };
        self.data = ptr::null_mut();
        self.length = 0;
        NativeStatus::checked(status)
    }
}

#[cfg(target_os = "macos")]
impl Drop for NativePasswordBuffer {
    fn drop(&mut self) {
        let _ = self.release_checked();
    }
}

#[cfg(target_os = "macos")]
impl SecurityFrameworkCore {
    pub(in crate::credentials::adapters) const fn new() -> Self {
        Self
    }

    fn checked_length(length: usize) -> Result<u32, NativeStatus> {
        u32::try_from(length).map_err(|_| NativeStatus::DATA_TOO_LARGE)
    }

    pub(in crate::credentials::adapters) fn interaction_allowed(
        &self,
    ) -> Result<bool, NativeStatus> {
        let mut allowed: Boolean = 0;
        let status = unsafe { SecKeychainGetUserInteractionAllowed(&mut allowed) };
        NativeStatus::checked(status)?;
        Ok(allowed != 0)
    }

    pub(in crate::credentials::adapters) fn set_interaction_allowed(
        &self,
        allowed: bool,
    ) -> Result<(), NativeStatus> {
        let status = unsafe { SecKeychainSetUserInteractionAllowed(Boolean::from(allowed)) };
        NativeStatus::checked(status)
    }

    pub(in crate::credentials::adapters) fn default_user_keychain(
        &self,
    ) -> Result<UserKeychain, NativeStatus> {
        let mut raw_keychain: SecKeychainRef = ptr::null_mut();
        let status =
            unsafe { SecKeychainCopyDomainDefault(SecPreferencesDomain::User, &mut raw_keychain) };
        NativeStatus::checked(status)?;
        if raw_keychain.is_null() {
            return Err(NativeStatus::INTERNAL_COMPONENT);
        }
        let keychain = unsafe { SecKeychain::wrap_under_create_rule(raw_keychain) };
        Ok(UserKeychain(keychain))
    }

    pub(in crate::credentials::adapters) fn read_secret(
        &self,
        keychain: &UserKeychain,
        service: &str,
        account: &str,
    ) -> Result<Zeroizing<Vec<u8>>, NativeStatus> {
        let service_length = Self::checked_length(service.len())?;
        let account_length = Self::checked_length(account.len())?;
        let mut password_length = 0u32;
        let mut password_data = ptr::null_mut();
        let mut raw_item: SecKeychainItemRef = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                keychain.0.as_CFTypeRef(),
                service_length,
                service.as_ptr().cast(),
                account_length,
                account.as_ptr().cast(),
                &mut password_length,
                &mut password_data,
                &mut raw_item,
            )
        };
        let mut password = NativePasswordBuffer::new(password_data, password_length);
        let item = if raw_item.is_null() {
            None
        } else {
            Some(KeychainItem(unsafe {
                SecKeychainItem::wrap_under_create_rule(raw_item)
            }))
        };

        if status != NativeStatus::SUCCESS.0 {
            let cleanup = password.release_checked();
            drop(item);
            cleanup?;
            return Err(NativeStatus::from_raw(status));
        }
        let Some(_item) = item else {
            password.release_checked()?;
            return Err(NativeStatus::INTERNAL_COMPONENT);
        };
        password.copy_and_release()
    }

    pub(in crate::credentials::adapters) fn find_item(
        &self,
        keychain: &UserKeychain,
        service: &str,
        account: &str,
    ) -> Result<KeychainItem, NativeStatus> {
        let service_length = Self::checked_length(service.len())?;
        let account_length = Self::checked_length(account.len())?;
        let mut raw_item: SecKeychainItemRef = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                keychain.0.as_CFTypeRef(),
                service_length,
                service.as_ptr().cast(),
                account_length,
                account.as_ptr().cast(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut raw_item,
            )
        };
        if status != NativeStatus::SUCCESS.0 {
            if !raw_item.is_null() {
                drop(KeychainItem(unsafe {
                    SecKeychainItem::wrap_under_create_rule(raw_item)
                }));
            }
            return Err(NativeStatus::from_raw(status));
        }
        if raw_item.is_null() {
            return Err(NativeStatus::INTERNAL_COMPONENT);
        }
        Ok(KeychainItem(unsafe {
            SecKeychainItem::wrap_under_create_rule(raw_item)
        }))
    }

    pub(in crate::credentials::adapters) fn add_secret(
        &self,
        keychain: &UserKeychain,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), NativeStatus> {
        let status = unsafe {
            SecKeychainAddGenericPassword(
                keychain.0.as_concrete_TypeRef(),
                Self::checked_length(service.len())?,
                service.as_ptr().cast(),
                Self::checked_length(account.len())?,
                account.as_ptr().cast(),
                Self::checked_length(secret.len())?,
                secret.as_ptr().cast(),
                ptr::null_mut(),
            )
        };
        NativeStatus::checked(status)
    }

    pub(in crate::credentials::adapters) fn update_secret(
        &self,
        item: &KeychainItem,
        secret: &[u8],
    ) -> Result<(), NativeStatus> {
        let status = unsafe {
            SecKeychainItemModifyAttributesAndData(
                item.0.as_concrete_TypeRef(),
                ptr::null(),
                Self::checked_length(secret.len())?,
                secret.as_ptr().cast(),
            )
        };
        NativeStatus::checked(status)
    }

    pub(in crate::credentials::adapters) fn delete_item(
        &self,
        item: &KeychainItem,
    ) -> Result<(), NativeStatus> {
        let status = unsafe { SecKeychainItemDelete(item.0.as_concrete_TypeRef()) };
        NativeStatus::checked(status)
    }

    pub(in crate::credentials::adapters) fn unlock(
        &self,
        keychain: &UserKeychain,
    ) -> Result<(), NativeStatus> {
        let status = unsafe {
            SecKeychainUnlock(
                keychain.0.as_concrete_TypeRef(),
                0,
                ptr::null(),
                Boolean::from(false),
            )
        };
        NativeStatus::checked(status)
    }

    pub(in crate::credentials::adapters) fn status(
        &self,
        keychain: &UserKeychain,
    ) -> Result<KeychainStatus, NativeStatus> {
        let mut status_bits = 0u32;
        let status =
            unsafe { SecKeychainGetStatus(keychain.0.as_concrete_TypeRef(), &mut status_bits) };
        NativeStatus::checked(status)?;
        Ok(KeychainStatus {
            unlocked: status_bits & UNLOCKED_STATUS_BIT != 0,
            readable: status_bits & READABLE_STATUS_BIT != 0,
            writable: status_bits & WRITABLE_STATUS_BIT != 0,
        })
    }
}
