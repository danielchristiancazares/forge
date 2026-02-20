use std::io;
use std::path::Path;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{
    ACL, CopySid, DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, NO_INHERITANCE,
    PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[cfg(windows)]
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

#[cfg(windows)]
struct TokenHandle(HANDLE);

#[cfg(windows)]
impl Drop for TokenHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn to_wide_null(value: &OsStr) -> Vec<u16> {
    let mut wide: Vec<u16> = value.encode_wide().collect();
    wide.push(0);
    wide
}

#[cfg(windows)]
fn current_user_sid_copy() -> io::Result<Vec<u8>> {
    let mut raw_token: HANDLE = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw_token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _token = TokenHandle(raw_token);

    let mut needed = 0u32;
    unsafe {
        let _ = GetTokenInformation(raw_token, TokenUser, ptr::null_mut(), 0, &raw mut needed);
    }
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut token_buf = vec![0u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            raw_token,
            TokenUser,
            token_buf.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    let token_user = unsafe { ptr::read_unaligned(token_buf.as_ptr().cast::<TOKEN_USER>()) };
    let sid_len = unsafe { GetLengthSid(token_user.User.Sid) };
    if sid_len == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut sid = vec![0u8; sid_len as usize];
    if unsafe { CopySid(sid_len, sid.as_mut_ptr().cast(), token_user.User.Sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}

#[cfg(windows)]
fn set_owner_only_acl(path: &Path, is_directory: bool) -> io::Result<()> {
    let mut sid = current_user_sid_copy()?;

    let explicit_access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if is_directory {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid.as_mut_ptr().cast(),
        },
    };

    let mut acl: *mut ACL = ptr::null_mut();
    let acl_status =
        unsafe { SetEntriesInAclW(1, &raw const explicit_access, ptr::null_mut(), &raw mut acl) };
    if acl_status != 0 {
        return Err(io::Error::from_raw_os_error(acl_status as i32));
    }

    let mut wide_path = to_wide_null(path.as_os_str());
    let set_status = unsafe {
        SetNamedSecurityInfoW(
            wide_path.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };

    unsafe {
        let _ = LocalFree(acl.cast());
    }

    if set_status != 0 {
        return Err(io::Error::from_raw_os_error(set_status as i32));
    }
    Ok(())
}

#[cfg(windows)]
pub fn set_owner_only_file_acl(path: &Path) -> io::Result<()> {
    set_owner_only_acl(path, false)
}

#[cfg(windows)]
pub fn set_owner_only_dir_acl(path: &Path) -> io::Result<()> {
    set_owner_only_acl(path, true)
}

#[cfg(not(windows))]
pub fn set_owner_only_file_acl(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn set_owner_only_dir_acl(_path: &Path) -> io::Result<()> {
    Ok(())
}
