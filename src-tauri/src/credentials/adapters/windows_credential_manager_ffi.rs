use std::ffi::c_void;
use std::ptr::NonNull;
use zeroize::{Zeroize, Zeroizing};

const MAX_TARGET_UTF16_UNITS: usize = 32_767;
const MAX_CREDENTIAL_BLOB_BYTES: usize = 2_560;

pub(super) struct PreparedTarget {
    wide: Zeroizing<Vec<u16>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrepareFailure {
    InvalidTarget,
    PayloadTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadFailure {
    AccessDenied,
    Missing,
    Cancelled,
    Unavailable,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteFailure {
    AccessDenied,
    Cancelled,
    Unavailable,
    CommitUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeleteFailure {
    AccessDenied,
    Missing,
    Cancelled,
    Unavailable,
    CommitUnknown,
}

pub(super) struct PreparedWrite {
    target: PreparedTarget,
    secret: Zeroizing<Vec<u8>>,
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeCredentialType {
    Generic,
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeCredentialPersistence {
    LocalMachine,
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeWriteRequestFields {
    flags: u32,
    credential_type: NativeCredentialType,
    target_name: *mut u16,
    comment: *mut u16,
    credential_blob_size: u32,
    credential_blob: *mut u8,
    persist: NativeCredentialPersistence,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
    write_flags: u32,
}

#[cfg(any(test, target_os = "windows"))]
impl PreparedWrite {
    fn native_request_fields(&mut self) -> NativeWriteRequestFields {
        NativeWriteRequestFields {
            flags: 0,
            credential_type: NativeCredentialType::Generic,
            target_name: self.target.wide.as_mut_ptr(),
            comment: std::ptr::null_mut(),
            credential_blob_size: self.secret.len() as u32,
            credential_blob: if self.secret.is_empty() {
                std::ptr::null_mut()
            } else {
                self.secret.as_mut_ptr()
            },
            persist: NativeCredentialPersistence::LocalMachine,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null_mut(),
            user_name: std::ptr::null_mut(),
            write_flags: 0,
        }
    }
}

pub(super) trait RawCredentialStore: Send + Sync {
    fn read(&self, target: PreparedTarget) -> Result<Zeroizing<Vec<u8>>, ReadFailure>;
    fn write(&self, request: PreparedWrite) -> Result<(), WriteFailure>;
    fn delete(&self, target: PreparedTarget) -> Result<(), DeleteFailure>;
}

type FreeNativeAllocation = unsafe fn(NonNull<c_void>);

struct NativeCredentialBuffer {
    allocation: NonNull<c_void>,
    blob: Option<NonNull<u8>>,
    length: usize,
    free: FreeNativeAllocation,
}

impl NativeCredentialBuffer {
    unsafe fn from_raw_parts(
        allocation: NonNull<c_void>,
        blob: Option<NonNull<u8>>,
        length: usize,
        free: FreeNativeAllocation,
    ) -> Self {
        Self {
            allocation,
            blob,
            length,
            free,
        }
    }

    fn copy_secret(&self) -> Result<Zeroizing<Vec<u8>>, ReadFailure> {
        if self.length > MAX_CREDENTIAL_BLOB_BYTES {
            return Err(ReadFailure::Internal);
        }
        if self.length == 0 {
            return Ok(Zeroizing::new(Vec::new()));
        }
        let blob = self.blob.ok_or(ReadFailure::Internal)?;
        let source = unsafe { std::slice::from_raw_parts(blob.as_ptr(), self.length) };
        Ok(Zeroizing::new(source.to_vec()))
    }
}

impl Drop for NativeCredentialBuffer {
    fn drop(&mut self) {
        if self.length != 0
            && let Some(blob) = self.blob
        {
            let secret = unsafe { std::slice::from_raw_parts_mut(blob.as_ptr(), self.length) };
            secret.zeroize();
        }
        unsafe { (self.free)(self.allocation) };
    }
}

pub(super) fn prepare_target(target: &str) -> Result<PreparedTarget, PrepareFailure> {
    if target.is_empty() || target.contains('\0') {
        return Err(PrepareFailure::InvalidTarget);
    }
    let units = target.encode_utf16().count();
    if units > MAX_TARGET_UTF16_UNITS {
        return Err(PrepareFailure::InvalidTarget);
    }
    let mut wide = Zeroizing::new(Vec::with_capacity(units + 1));
    wide.extend(target.encode_utf16());
    wide.push(0);
    Ok(PreparedTarget { wide })
}

pub(super) fn prepare_write(target: &str, secret: &[u8]) -> Result<PreparedWrite, PrepareFailure> {
    let target = prepare_target(target)?;
    if secret.len() > MAX_CREDENTIAL_BLOB_BYTES {
        return Err(PrepareFailure::PayloadTooLarge);
    }
    Ok(PreparedWrite {
        target,
        secret: Zeroizing::new(secret.to_vec()),
    })
}

fn map_read_failure(code: u32) -> ReadFailure {
    match code {
        5 => ReadFailure::AccessDenied,
        1168 => ReadFailure::Missing,
        1223 => ReadFailure::Cancelled,
        1312 => ReadFailure::Unavailable,
        _ => ReadFailure::Internal,
    }
}

fn map_write_failure(code: u32) -> WriteFailure {
    match code {
        5 => WriteFailure::AccessDenied,
        1223 => WriteFailure::Cancelled,
        1312 => WriteFailure::Unavailable,
        _ => WriteFailure::CommitUnknown,
    }
}

fn map_delete_failure(code: u32) -> DeleteFailure {
    match code {
        5 => DeleteFailure::AccessDenied,
        1168 => DeleteFailure::Missing,
        1223 => DeleteFailure::Cancelled,
        1312 => DeleteFailure::Unavailable,
        _ => DeleteFailure::CommitUnknown,
    }
}

#[cfg(target_os = "windows")]
pub(super) struct NativeWinCredStore;

#[cfg(target_os = "windows")]
impl NativeWinCredStore {
    pub(super) fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
mod native_wincred {
    use super::{
        DeleteFailure, NativeCredentialBuffer, NativeCredentialPersistence, NativeCredentialType,
        NativeWinCredStore, PreparedTarget, PreparedWrite, RawCredentialStore, ReadFailure,
        WriteFailure, map_delete_failure, map_read_failure, map_write_failure,
    };
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE, CRED_TYPE_GENERIC, CREDENTIALW,
    };
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use zeroize::Zeroizing;

    windows::core::link!("advapi32.dll" "system" "CredReadW" fn cred_read_w(
        target_name: PCWSTR,
        credential_type: CRED_TYPE,
        flags: u32,
        credential: *mut *mut CREDENTIALW,
    ) -> BOOL);
    windows::core::link!("advapi32.dll" "system" "CredWriteW" fn cred_write_w(
        credential: *const CREDENTIALW,
        flags: u32,
    ) -> BOOL);
    windows::core::link!("advapi32.dll" "system" "CredDeleteW" fn cred_delete_w(
        target_name: PCWSTR,
        credential_type: CRED_TYPE,
        flags: u32,
    ) -> BOOL);
    windows::core::link!("advapi32.dll" "system" "CredFree" fn cred_free(
        buffer: *const c_void,
    ));

    unsafe fn free_native_credential(allocation: NonNull<c_void>) {
        unsafe { cred_free(allocation.as_ptr()) };
    }

    impl RawCredentialStore for NativeWinCredStore {
        fn read(&self, target: PreparedTarget) -> Result<Zeroizing<Vec<u8>>, ReadFailure> {
            let mut credential = std::ptr::null_mut();
            let status = unsafe {
                cred_read_w(
                    PCWSTR(target.wide.as_ptr()),
                    CRED_TYPE_GENERIC,
                    0,
                    &mut credential,
                )
            };
            if !status.as_bool() {
                let code = unsafe { GetLastError() }.0;
                return Err(map_read_failure(code));
            }
            let credential = NonNull::new(credential).ok_or(ReadFailure::Internal)?;
            let native = unsafe { credential.as_ref() };
            let buffer = unsafe {
                NativeCredentialBuffer::from_raw_parts(
                    credential.cast(),
                    NonNull::new(native.CredentialBlob),
                    native.CredentialBlobSize as usize,
                    free_native_credential,
                )
            };
            buffer.copy_secret()
        }

        fn write(&self, mut request: PreparedWrite) -> Result<(), WriteFailure> {
            let fields = request.native_request_fields();
            let mut credential = CREDENTIALW {
                Flags: CRED_FLAGS(fields.flags),
                Type: match fields.credential_type {
                    NativeCredentialType::Generic => CRED_TYPE_GENERIC,
                },
                TargetName: PWSTR(fields.target_name),
                Comment: PWSTR(fields.comment),
                LastWritten: Default::default(),
                CredentialBlobSize: fields.credential_blob_size,
                CredentialBlob: fields.credential_blob,
                Persist: match fields.persist {
                    NativeCredentialPersistence::LocalMachine => CRED_PERSIST_LOCAL_MACHINE,
                },
                AttributeCount: fields.attribute_count,
                Attributes: fields.attributes.cast(),
                TargetAlias: PWSTR(fields.target_alias),
                UserName: PWSTR(fields.user_name),
            };
            let status = unsafe { cred_write_w(&mut credential, fields.write_flags) };
            if !status.as_bool() {
                let code = unsafe { GetLastError() }.0;
                return Err(map_write_failure(code));
            }
            Ok(())
        }

        fn delete(&self, target: PreparedTarget) -> Result<(), DeleteFailure> {
            let status =
                unsafe { cred_delete_w(PCWSTR(target.wide.as_ptr()), CRED_TYPE_GENERIC, 0) };
            if !status.as_bool() {
                let code = unsafe { GetLastError() }.0;
                return Err(map_delete_failure(code));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::{
        DeleteFailure, NativeCredentialBuffer, NativeCredentialPersistence, NativeCredentialType,
        NativeWriteRequestFields, PreparedTarget, PreparedWrite, RawCredentialStore, ReadFailure,
        WriteFailure, map_delete_failure, map_read_failure, map_write_failure, prepare_target,
        prepare_write,
    };
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zeroize::Zeroizing;

    #[derive(Clone, Copy)]
    struct RustToken<'source> {
        text: &'source str,
        start: usize,
    }

    fn is_identifier_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_'
    }

    fn blank_non_code(masked: &mut [u8], start: usize, end: usize) {
        for byte in &mut masked[start..end] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
    }

    fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            return None;
        }
        let prefix_len = if bytes[start..].starts_with(b"br") || bytes[start..].starts_with(b"cr") {
            2
        } else if bytes[start] == b'r' {
            1
        } else {
            return None;
        };
        let mut quote = start + prefix_len;
        while bytes.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if bytes.get(quote) != Some(&b'"') {
            return None;
        }
        let hashes = quote - start - prefix_len;
        let mut cursor = quote + 1;
        while cursor < bytes.len() {
            let closing_end = cursor + 1 + hashes;
            if bytes[cursor] == b'"'
                && closing_end <= bytes.len()
                && bytes[cursor + 1..closing_end]
                    .iter()
                    .all(|byte| *byte == b'#')
            {
                return Some(closing_end);
            }
            cursor += 1;
        }
        Some(bytes.len())
    }

    fn quoted_string_end(bytes: &[u8], start: usize) -> Option<usize> {
        let quote = if bytes[start] == b'"' {
            start
        } else if start == 0 || !is_identifier_byte(bytes[start - 1]) {
            if matches!(bytes[start], b'b' | b'c') && bytes.get(start + 1) == Some(&b'"') {
                start + 1
            } else {
                return None;
            }
        } else {
            return None;
        };
        let mut cursor = quote + 1;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'\\' => cursor = (cursor + 2).min(bytes.len()),
                b'"' => return Some(cursor + 1),
                _ => cursor += 1,
            }
        }
        Some(bytes.len())
    }

    fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
        let quote = if bytes[start] == b'\'' {
            start
        } else if bytes[start] == b'b'
            && (start == 0 || !is_identifier_byte(bytes[start - 1]))
            && bytes.get(start + 1) == Some(&b'\'')
        {
            start + 1
        } else {
            return None;
        };
        let content = quote + 1;
        if bytes.get(content) == Some(&b'\\') {
            let mut cursor = content + 2;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\'' | b'\n' | b'\r') {
                cursor += 1;
            }
            return (bytes.get(cursor) == Some(&b'\'')).then_some(cursor + 1);
        }
        let character = std::str::from_utf8(bytes.get(content..)?)
            .ok()?
            .chars()
            .next()?;
        let closing = content + character.len_utf8();
        (bytes.get(closing) == Some(&b'\'')).then_some(closing + 1)
    }

    fn mask_rust(source: &str, preserve_strings: bool) -> String {
        let bytes = source.as_bytes();
        let mut masked = bytes.to_vec();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor..].starts_with(b"//") {
                let end = bytes[cursor..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| cursor + offset);
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if bytes[cursor..].starts_with(b"/*") {
                let mut end = cursor + 2;
                let mut depth = 1_usize;
                while end < bytes.len() && depth > 0 {
                    if bytes[end..].starts_with(b"/*") {
                        depth += 1;
                        end += 2;
                    } else if bytes[end..].starts_with(b"*/") {
                        depth -= 1;
                        end += 2;
                    } else {
                        end += 1;
                    }
                }
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if let Some(end) = raw_string_end(bytes, cursor) {
                if !preserve_strings {
                    blank_non_code(&mut masked, cursor, end);
                }
                cursor = end;
            } else if let Some(end) = quoted_string_end(bytes, cursor) {
                if !preserve_strings {
                    blank_non_code(&mut masked, cursor, end);
                }
                cursor = end;
            } else if let Some(end) = char_literal_end(bytes, cursor) {
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else {
                cursor += source[cursor..]
                    .chars()
                    .next()
                    .expect("cursor remains on a UTF-8 boundary")
                    .len_utf8();
            }
        }
        String::from_utf8(masked).expect("masking preserves UTF-8")
    }

    fn mask_rust_non_code(source: &str) -> String {
        mask_rust(source, false)
    }

    fn mask_rust_comments_and_chars(source: &str) -> String {
        mask_rust(source, true)
    }

    fn rust_tokens(source: &str) -> Vec<RustToken<'_>> {
        let bytes = source.as_bytes();
        let mut tokens = Vec::new();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            } else if bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len() && is_identifier_byte(bytes[cursor]) {
                    cursor += 1;
                }
                tokens.push(RustToken {
                    text: &source[start..cursor],
                    start,
                });
            } else {
                let start = cursor;
                cursor += source[cursor..]
                    .chars()
                    .next()
                    .expect("cursor remains on a UTF-8 boundary")
                    .len_utf8();
                tokens.push(RustToken {
                    text: &source[start..cursor],
                    start,
                });
            }
        }
        tokens
    }

    fn matching_delimiter(
        tokens: &[RustToken<'_>],
        open: usize,
        open_text: &str,
        close_text: &str,
    ) -> Option<usize> {
        if tokens.get(open)?.text != open_text {
            return None;
        }
        let mut depth = 0_usize;
        for (index, token) in tokens.iter().enumerate().skip(open) {
            if token.text == open_text {
                depth += 1;
            } else if token.text == close_text {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
        None
    }

    fn root_test_module_range(source: &str, module_name: &str) -> Option<(usize, usize)> {
        const CFG_TEST: [&str; 7] = ["#", "[", "cfg", "(", "test", ")", "]"];
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let mut brace_depth = 0_usize;
        let mut found = None;
        let mut index = 0_usize;
        while index < tokens.len() {
            if brace_depth == 0
                && tokens[index..]
                    .iter()
                    .take(CFG_TEST.len())
                    .map(|token| token.text)
                    .eq(CFG_TEST)
            {
                let mut cursor = index + CFG_TEST.len();
                if tokens.get(cursor).is_some_and(|token| token.text == "pub") {
                    cursor += 1;
                    if tokens.get(cursor).is_some_and(|token| token.text == "(") {
                        cursor = matching_delimiter(&tokens, cursor, "(", ")")? + 1;
                    }
                }
                if tokens.get(cursor).is_some_and(|token| token.text == "mod")
                    && tokens
                        .get(cursor + 1)
                        .is_some_and(|token| token.text == module_name)
                    && tokens
                        .get(cursor + 2)
                        .is_some_and(|token| token.text == "{")
                {
                    let close = matching_delimiter(&tokens, cursor + 2, "{", "}")?;
                    if found.is_some() {
                        return None;
                    }
                    found = Some((tokens[index].start, tokens[close].start + 1));
                    index = close + 1;
                    continue;
                }
            }
            match tokens[index].text {
                "{" => brace_depth += 1,
                "}" => brace_depth = brace_depth.checked_sub(1)?,
                _ => {}
            }
            index += 1;
        }
        (brace_depth == 0).then_some(found).flatten()
    }

    fn production_source(source: &str, test_module_name: &str) -> Option<String> {
        let (start, end) = root_test_module_range(source, test_module_name)?;
        let mut production = source.as_bytes().to_vec();
        blank_non_code(&mut production, start, end);
        String::from_utf8(production).ok()
    }

    pub(in crate::credentials::adapters) fn masked_production_code(
        source: &str,
        test_module_name: &str,
    ) -> Option<String> {
        production_source(source, test_module_name).map(|source| mask_rust_non_code(&source))
    }

    pub(in crate::credentials::adapters) fn production_code_with_strings(
        source: &str,
        test_module_name: &str,
    ) -> Option<String> {
        production_source(source, test_module_name)
            .map(|source| mask_rust_comments_and_chars(&source))
    }

    pub(in crate::credentials::adapters) fn masked_rust_code(source: &str) -> String {
        mask_rust_non_code(source)
    }

    pub(in crate::credentials::adapters) fn rust_code_with_strings(source: &str) -> String {
        mask_rust_comments_and_chars(source)
    }

    #[derive(Default)]
    pub(in crate::credentials::adapters) struct CallCounts {
        reads: AtomicUsize,
        writes: AtomicUsize,
        deletes: AtomicUsize,
    }

    impl CallCounts {
        pub(in crate::credentials::adapters) fn snapshot(&self) -> (usize, usize, usize) {
            (
                self.reads.load(Ordering::SeqCst),
                self.writes.load(Ordering::SeqCst),
                self.deletes.load(Ordering::SeqCst),
            )
        }
    }

    struct UnknownReadStore {
        code: u32,
        counts: Arc<CallCounts>,
    }

    #[derive(Default)]
    struct MemoryStore {
        entries: Mutex<HashMap<Vec<u16>, Zeroizing<Vec<u8>>>>,
        counts: Arc<CallCounts>,
    }

    struct FailingMutationStore {
        write_code: Option<u32>,
        delete_code: Option<u32>,
        counts: Arc<CallCounts>,
    }

    #[derive(Default)]
    struct NativeBufferObservation {
        scrubbed_before_free: AtomicUsize,
        frees: AtomicUsize,
    }

    impl NativeBufferObservation {
        fn snapshot(&self) -> (usize, usize) {
            (
                self.scrubbed_before_free.load(Ordering::SeqCst),
                self.frees.load(Ordering::SeqCst),
            )
        }
    }

    struct ObservedNativeAllocation {
        bytes: Vec<u8>,
        observation: Arc<NativeBufferObservation>,
    }

    unsafe fn free_observed_native_allocation(allocation: NonNull<c_void>) {
        let allocation =
            unsafe { Box::from_raw(allocation.cast::<ObservedNativeAllocation>().as_ptr()) };
        if allocation.bytes.iter().all(|byte| *byte == 0) {
            allocation
                .observation
                .scrubbed_before_free
                .fetch_add(1, Ordering::SeqCst);
        }
        allocation.observation.frees.fetch_add(1, Ordering::SeqCst);
    }

    fn observed_native_buffer(
        bytes: Vec<u8>,
    ) -> (NativeCredentialBuffer, Arc<NativeBufferObservation>) {
        let length = bytes.len();
        observed_native_buffer_parts(bytes, length, true)
    }

    fn observed_native_buffer_parts(
        bytes: Vec<u8>,
        length: usize,
        has_blob: bool,
    ) -> (NativeCredentialBuffer, Arc<NativeBufferObservation>) {
        let observation = Arc::new(NativeBufferObservation::default());
        let mut allocation = Box::new(ObservedNativeAllocation {
            bytes,
            observation: observation.clone(),
        });
        let blob = has_blob
            .then(|| NonNull::new(allocation.bytes.as_mut_ptr()))
            .flatten();
        let allocation = NonNull::new(Box::into_raw(allocation).cast::<c_void>())
            .expect("boxed test native allocation");
        let guard = unsafe {
            NativeCredentialBuffer::from_raw_parts(
                allocation,
                blob,
                length,
                free_observed_native_allocation,
            )
        };
        (guard, observation)
    }

    impl RawCredentialStore for FailingMutationStore {
        fn read(&self, _target: PreparedTarget) -> Result<Zeroizing<Vec<u8>>, ReadFailure> {
            self.counts.reads.fetch_add(1, Ordering::SeqCst);
            Err(ReadFailure::Missing)
        }

        fn write(&self, _request: PreparedWrite) -> Result<(), WriteFailure> {
            self.counts.writes.fetch_add(1, Ordering::SeqCst);
            self.write_code
                .map_or(Ok(()), |code| Err(map_write_failure(code)))
        }

        fn delete(&self, _target: PreparedTarget) -> Result<(), DeleteFailure> {
            self.counts.deletes.fetch_add(1, Ordering::SeqCst);
            self.delete_code
                .map_or(Ok(()), |code| Err(map_delete_failure(code)))
        }
    }

    impl RawCredentialStore for MemoryStore {
        fn read(&self, target: PreparedTarget) -> Result<Zeroizing<Vec<u8>>, ReadFailure> {
            self.counts.reads.fetch_add(1, Ordering::SeqCst);
            self.entries
                .lock()
                .expect("memory WinCred lock")
                .get(target.wide.as_slice())
                .cloned()
                .ok_or(ReadFailure::Missing)
        }

        fn write(&self, request: PreparedWrite) -> Result<(), WriteFailure> {
            self.counts.writes.fetch_add(1, Ordering::SeqCst);
            self.entries
                .lock()
                .expect("memory WinCred lock")
                .insert(request.target.wide.to_vec(), request.secret.clone());
            Ok(())
        }

        fn delete(&self, target: PreparedTarget) -> Result<(), DeleteFailure> {
            self.counts.deletes.fetch_add(1, Ordering::SeqCst);
            self.entries
                .lock()
                .expect("memory WinCred lock")
                .remove(target.wide.as_slice())
                .map(|_| ())
                .ok_or(DeleteFailure::Missing)
        }
    }

    impl RawCredentialStore for UnknownReadStore {
        fn read(&self, _target: PreparedTarget) -> Result<Zeroizing<Vec<u8>>, ReadFailure> {
            self.counts.reads.fetch_add(1, Ordering::SeqCst);
            Err(map_read_failure(self.code))
        }

        fn write(&self, _request: PreparedWrite) -> Result<(), WriteFailure> {
            self.counts.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn delete(&self, _target: PreparedTarget) -> Result<(), DeleteFailure> {
            self.counts.deletes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    pub(in crate::credentials::adapters) fn unknown_read_store(
        code: u32,
    ) -> (Arc<dyn RawCredentialStore>, Arc<CallCounts>) {
        let counts = Arc::new(CallCounts::default());
        (
            Arc::new(UnknownReadStore {
                code,
                counts: counts.clone(),
            }),
            counts,
        )
    }

    pub(in crate::credentials::adapters) fn memory_store()
    -> (Arc<dyn RawCredentialStore>, Arc<CallCounts>) {
        let counts = Arc::new(CallCounts::default());
        (
            Arc::new(MemoryStore {
                entries: Mutex::new(HashMap::new()),
                counts: counts.clone(),
            }),
            counts,
        )
    }

    pub(in crate::credentials::adapters) fn failing_mutation_store(
        write_code: Option<u32>,
        delete_code: Option<u32>,
    ) -> (Arc<dyn RawCredentialStore>, Arc<CallCounts>) {
        let counts = Arc::new(CallCounts::default());
        (
            Arc::new(FailingMutationStore {
                write_code,
                delete_code,
                counts: counts.clone(),
            }),
            counts,
        )
    }

    #[test]
    fn numeric_status_matrix_is_closed_per_operation() {
        assert_eq!(map_read_failure(5), ReadFailure::AccessDenied);
        assert_eq!(map_read_failure(1168), ReadFailure::Missing);
        assert_eq!(map_read_failure(1223), ReadFailure::Cancelled);
        assert_eq!(map_read_failure(1312), ReadFailure::Unavailable);
        assert_eq!(map_read_failure(0xfefe_fefe), ReadFailure::Internal);

        assert_eq!(map_write_failure(5), WriteFailure::AccessDenied);
        assert_eq!(map_write_failure(1168), WriteFailure::CommitUnknown);
        assert_eq!(map_write_failure(1223), WriteFailure::Cancelled);
        assert_eq!(map_write_failure(1312), WriteFailure::Unavailable);
        assert_eq!(map_write_failure(0xfefe_fefe), WriteFailure::CommitUnknown);

        assert_eq!(map_delete_failure(5), DeleteFailure::AccessDenied);
        assert_eq!(map_delete_failure(1168), DeleteFailure::Missing);
        assert_eq!(map_delete_failure(1223), DeleteFailure::Cancelled);
        assert_eq!(map_delete_failure(1312), DeleteFailure::Unavailable);
        assert_eq!(
            map_delete_failure(0xfefe_fefe),
            DeleteFailure::CommitUnknown
        );
    }

    #[test]
    fn request_bounds_are_utf16_exact_and_binary_inclusive() {
        assert!(matches!(
            prepare_target(""),
            Err(super::PrepareFailure::InvalidTarget)
        ));
        assert!(matches!(
            prepare_target("prefix\0suffix"),
            Err(super::PrepareFailure::InvalidTarget)
        ));

        let exact_target = "a".repeat(32_767);
        let exact = prepare_target(&exact_target).expect("32,767 UTF-16 units accepted");
        assert_eq!(exact.wide.len(), 32_768);
        assert_eq!(exact.wide.last(), Some(&0));
        assert!(matches!(
            prepare_target(&"a".repeat(32_768)),
            Err(super::PrepareFailure::InvalidTarget)
        ));

        let exact_surrogate_units = format!("{}a", "💣".repeat(16_383));
        assert_eq!(exact_surrogate_units.encode_utf16().count(), 32_767);
        assert!(prepare_target(&exact_surrogate_units).is_ok());
        assert!(matches!(
            prepare_target(&format!("{exact_surrogate_units}b")),
            Err(super::PrepareFailure::InvalidTarget)
        ));

        let exact_blob = vec![0xa5; 2_560];
        let prepared = prepare_write("target", &exact_blob).expect("2,560 bytes accepted");
        assert_eq!(prepared.secret.as_slice(), exact_blob.as_slice());
        assert!(matches!(
            prepare_write("target", &vec![0xa5; 2_561]),
            Err(super::PrepareFailure::PayloadTooLarge)
        ));
    }

    #[test]
    fn native_write_request_fields_are_exact_for_empty_and_nonempty_blobs() {
        let mut empty = prepare_write("target", &[]).expect("empty blob request");
        let empty_target = empty.target.wide.as_mut_ptr();
        assert_eq!(
            empty.native_request_fields(),
            NativeWriteRequestFields {
                flags: 0,
                credential_type: NativeCredentialType::Generic,
                target_name: empty_target,
                comment: std::ptr::null_mut(),
                credential_blob_size: 0,
                credential_blob: std::ptr::null_mut(),
                persist: NativeCredentialPersistence::LocalMachine,
                attribute_count: 0,
                attributes: std::ptr::null_mut(),
                target_alias: std::ptr::null_mut(),
                user_name: std::ptr::null_mut(),
                write_flags: 0,
            }
        );

        let mut nonempty =
            prepare_write("target", &[0x00, 0xa5, 0xff]).expect("nonempty blob request");
        let nonempty_target = nonempty.target.wide.as_mut_ptr();
        let nonempty_blob = nonempty.secret.as_mut_ptr();
        assert_eq!(
            nonempty.native_request_fields(),
            NativeWriteRequestFields {
                flags: 0,
                credential_type: NativeCredentialType::Generic,
                target_name: nonempty_target,
                comment: std::ptr::null_mut(),
                credential_blob_size: 3,
                credential_blob: nonempty_blob,
                persist: NativeCredentialPersistence::LocalMachine,
                attribute_count: 0,
                attributes: std::ptr::null_mut(),
                target_alias: std::ptr::null_mut(),
                user_name: std::ptr::null_mut(),
                write_flags: 0,
            }
        );
    }

    #[test]
    fn native_source_inventory_is_exact_and_capability_blind() {
        let source = include_str!("windows_credential_manager_ffi.rs");
        let production_source =
            production_source(source, "test_support").expect("one root test-support module");
        let production = mask_rust_non_code(&production_source);
        let compact: String = production
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();

        for (symbol, call) in [
            ("CredReadW", "cred_read_w"),
            ("CredWriteW", "cred_write_w"),
            ("CredDeleteW", "cred_delete_w"),
            ("CredFree", "cred_free"),
        ] {
            assert_eq!(
                production_source.matches(&format!("\"{symbol}\"")).count(),
                1,
                "exact link name for {symbol}"
            );
            assert_eq!(
                production_source.matches(symbol).count(),
                1,
                "sole native spelling for {symbol}"
            );
            assert_eq!(
                production.matches(call).count(),
                2,
                "one declaration and call"
            );
        }
        for forbidden in [
            "CredUI",
            "CredEnumerate",
            "CredRename",
            "ForbidPrompt",
            "AllowPrompt",
            "RecoveryBoundary",
            "MutationInvocation",
        ] {
            assert!(!production.contains(forbidden), "forbidden {forbidden}");
        }
        assert_eq!(production.matches("GetLastError").count(), 4);
        assert_eq!(compact.matches("windows::core::link!(").count(), 4);
        assert_eq!(
            compact
                .matches("implRawCredentialStoreforNativeWinCredStore")
                .count(),
            1
        );
        for mapper in [
            "map_read_failure(code)",
            "map_write_failure(code)",
            "map_delete_failure(code)",
        ] {
            assert!(
                compact.contains(&format!(
                    "if!status.as_bool(){{letcode=unsafe{{GetLastError()}}.0;returnErr({mapper});}}"
                )),
                "immediate error capture for {mapper}"
            );
        }
        for field in [
            "Flags:CRED_FLAGS(fields.flags)",
            "Type:matchfields.credential_type{NativeCredentialType::Generic=>CRED_TYPE_GENERIC,}",
            "TargetName:PWSTR(fields.target_name)",
            "Comment:PWSTR(fields.comment)",
            "LastWritten:Default::default()",
            "CredentialBlobSize:fields.credential_blob_size",
            "CredentialBlob:fields.credential_blob",
            "Persist:matchfields.persist{NativeCredentialPersistence::LocalMachine=>CRED_PERSIST_LOCAL_MACHINE,}",
            "AttributeCount:fields.attribute_count",
            "Attributes:fields.attributes.cast()",
            "TargetAlias:PWSTR(fields.target_alias)",
            "UserName:PWSTR(fields.user_name)",
        ] {
            assert!(compact.contains(field), "native field {field}");
        }
        assert!(!compact.contains("..CREDENTIALW::default()"));
        assert_eq!(compact.matches("cred_read_w(").count(), 2);
        assert_eq!(compact.matches("cred_write_w(").count(), 2);
        assert_eq!(compact.matches("cred_delete_w(").count(), 2);
        assert!(compact.contains(
            "cred_read_w(PCWSTR(target.wide.as_ptr()),CRED_TYPE_GENERIC,0,&mutcredential,)"
        ));
        assert!(compact.contains("cred_write_w(&mutcredential,fields.write_flags)"));
        assert!(
            compact.contains("cred_delete_w(PCWSTR(target.wide.as_ptr()),CRED_TYPE_GENERIC,0)")
        );
        for forbidden_flow in ["extern", "loop", "while", "retry"] {
            assert!(
                !production
                    .split(|character: char| {
                        !character.is_ascii_alphanumeric() && character != '_'
                    })
                    .any(|token| token == forbidden_flow),
                "forbidden raw flow {forbidden_flow}"
            );
        }
        assert!(compact.contains("secret.zeroize();"));
        assert!(compact.contains("unsafe{(self.free)(self.allocation)};"));
    }

    #[test]
    fn native_buffer_scrubs_before_one_free_on_success_error_and_unwind() {
        let (success, success_observed) = observed_native_buffer(vec![0xa5; 16]);
        let copied = success.copy_secret().expect("copy valid native blob");
        assert_eq!(copied.as_slice(), &[0xa5; 16]);
        drop(success);
        assert_eq!(success_observed.snapshot(), (1, 1));

        let (error, error_observed) = observed_native_buffer(vec![0xa5; 2_561]);
        assert_eq!(error.copy_secret(), Err(ReadFailure::Internal));
        drop(error);
        assert_eq!(error_observed.snapshot(), (1, 1));

        let (unwind, unwind_observed) = observed_native_buffer(vec![0xa5; 16]);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = unwind;
            panic!("non-secret native-buffer unwind canary");
        }));
        assert!(unwound.is_err());
        assert_eq!(unwind_observed.snapshot(), (1, 1));
    }

    #[test]
    fn native_null_blob_edges_return_closed_results_and_free_once() {
        let (empty, empty_observed) = observed_native_buffer_parts(Vec::new(), 0, false);
        assert_eq!(empty.copy_secret().expect("zero-length null blob").len(), 0);
        drop(empty);
        assert_eq!(empty_observed.snapshot().1, 1);

        let (invalid, invalid_observed) = observed_native_buffer_parts(vec![0xa5; 16], 16, false);
        assert_eq!(invalid.copy_secret(), Err(ReadFailure::Internal));
        drop(invalid);
        assert_eq!(invalid_observed.snapshot(), (0, 1));
    }
}
