//! Files only the user can read, on both platforms, from the moment they exist: mode 0600 (a
//! folder 0700) on the Mac; on Windows an access list of one entry, the user's, allowing
//! everything and inheriting nothing, where a new file would otherwise take its folder's (a data
//! folder moved to a second drive, a %TEMP% others share, whose folders let other accounts in).
//!
//! One place for it, used by every file Trek keeps a secret in (session secrets, the machine
//! list, the paired phones, an agent's MCP config, the pairing key, files uploaded from another
//! machine), so a secret can't end up private on one platform and not the other.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A new file at `path`, the user's alone before anything is written to it: on Windows it's made
/// with its access list, so no other account can open it in between. Fails if `path` exists.
pub fn create_new(path: &Path) -> io::Result<File> {
    imp::create_new(path)
}

/// `bytes` as the whole of `path`, the user's alone: written to a new private file beside it and
/// moved over it, so a reader sees the old contents or the new, and nobody else sees either.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = beside(path);
    let written = (|| {
        let mut file = create_new(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Make `path`, a file or a folder that's there, the user's alone (a folder: and what's made in it
/// after, which takes that on).
pub fn restrict(path: &Path) -> io::Result<()> {
    imp::restrict(path)
}

/// A folder at `path`, the user's alone, whether it was there or not; the parents it needs are
/// made as well.
pub fn create_dir(path: &Path) -> io::Result<()> {
    imp::create_dir(path)?;
    restrict(path)
}

/// Whether only the user can open `path`.
pub fn is_private(path: &Path) -> io::Result<bool> {
    imp::is_private(path)
}

/// The user this process runs as, as Windows writes a SID (`S-1-5-21-…`): for tests that read an
/// access list for themselves.
#[cfg(windows)]
pub fn user_sid() -> io::Result<String> {
    imp::user_sid()
}

/// A file beside `path` that no other writer names: `.<name>.<pid>.<n>.tmp`.
fn beside(path: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

#[cfg(unix)]
mod imp {
    use std::fs::{File, OpenOptions, Permissions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    use std::path::Path;

    pub fn create_new(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        // The umask can only take bits away from 0600; set it whole all the same.
        file.set_permissions(Permissions::from_mode(0o600))?;
        Ok(file)
    }

    pub fn restrict(path: &Path) -> io::Result<()> {
        let mode = if std::fs::metadata(path)?.is_dir() {
            0o700
        } else {
            0o600
        };
        std::fs::set_permissions(path, Permissions::from_mode(mode))
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }

    pub fn is_private(path: &Path) -> io::Result<bool> {
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o077 == 0)
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::FromRawHandle as _;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorControl,
        GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor, OBJECT_INHERIT_ACE,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SetSecurityDescriptorControl,
        SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// `ACCESS_ALLOWED_ACE_TYPE` (in a windows-sys module of its own).
    const ACCESS_ALLOWED: u8 = 0;
    /// `SECURITY_DESCRIPTOR_REVISION` (in SystemServices, a feature of its own).
    const DESCRIPTOR_REVISION: u32 = 1;

    /// The user this process runs as: its token's `TOKEN_USER`, in a buffer that holds the SID it
    /// points to.
    struct User(Vec<u64>);

    impl User {
        fn current() -> io::Result<User> {
            let mut token: HANDLE = null_mut();
            // SAFETY: this process's pseudo-handle, and a place for the token's handle.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            // SAFETY: a size query, with no buffer; `len` says what's needed.
            unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len) };
            // In u64s, so the TOKEN_USER at its start (a pointer first) is aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: `buf` holds at least `len` bytes.
            let ok = unsafe {
                GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
            };
            let failed = io::Error::last_os_error();
            // SAFETY: the handle opened above, closed once.
            unsafe { CloseHandle(token) };
            if ok == 0 {
                return Err(failed);
            }
            Ok(User(buf))
        }

        fn sid(&self) -> PSID {
            // SAFETY: the buffer starts with the TOKEN_USER that GetTokenInformation wrote, whose
            // SID is in the buffer too, alive as long as `self`.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    /// An access list of one entry: `user` may do everything. A folder's entry is passed on to
    /// what's made in it.
    struct Acl(Vec<u64>);

    impl Acl {
        fn only(user: PSID, folder: bool) -> io::Result<Acl> {
            // The entry is an ACCESS_ALLOWED_ACE whose last field is the SID's first four bytes.
            let entry = std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>();
            // SAFETY: a valid SID (the caller's).
            let size = std::mem::size_of::<ACL>() + entry + unsafe { GetLengthSid(user) } as usize;
            // In u64s: an ACL must be aligned to four bytes at least.
            let mut acl = Acl(vec![0u64; size.div_ceil(8)]);
            let flags = if folder {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                0
            };
            // SAFETY: the ACL is `size` bytes, and the SID is valid.
            unsafe {
                if InitializeAcl(acl.ptr(), size as u32, ACL_REVISION) == 0
                    || AddAccessAllowedAceEx(acl.ptr(), ACL_REVISION, flags, FILE_ALL_ACCESS, user)
                        == 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(acl)
        }

        fn ptr(&mut self) -> *mut ACL {
            self.0.as_mut_ptr().cast()
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    /// The security a new file or folder is made with: `acl` alone, inheriting nothing from its
    /// folder. Both must outlive the call that uses it.
    fn descriptor(acl: &mut Acl, descriptor: &mut SECURITY_DESCRIPTOR) -> io::Result<()> {
        let sd: PSECURITY_DESCRIPTOR = (descriptor as *mut SECURITY_DESCRIPTOR).cast();
        // SAFETY: a descriptor of the caller's to fill, and an ACL that outlives its use.
        let ok = unsafe {
            InitializeSecurityDescriptor(sd, DESCRIPTOR_REVISION) != 0
                && SetSecurityDescriptorDacl(sd, 1, acl.ptr(), 0) != 0
                && SetSecurityDescriptorControl(sd, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0
        };
        if ok {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn create_new(path: &Path) -> io::Result<File> {
        let user = User::current()?;
        let mut acl = Acl::only(user.sid(), false)?;
        let mut security = SECURITY_DESCRIPTOR::default();
        descriptor(&mut acl, &mut security)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: (&mut security as *mut SECURITY_DESCRIPTOR).cast(),
            bInheritHandle: 0,
        };
        let name = wide(path);
        // SAFETY: a NUL-ended name and the attributes above (their ACL alive until after).
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a file handle just opened, owned by the File from here on.
        Ok(unsafe { File::from_raw_handle(handle) })
    }

    pub fn restrict(path: &Path) -> io::Result<()> {
        let folder = std::fs::metadata(path)?.is_dir();
        let user = User::current()?;
        if is_only(path, user.sid())? {
            return Ok(());
        }
        let mut acl = Acl::only(user.sid(), folder)?;
        let name = wide(path);
        // SAFETY: a NUL-ended name, and the ACL built above; owner, group and SACL are left.
        let e = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl.ptr(),
                null(),
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        Ok(())
    }

    pub fn create_dir(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn is_private(path: &Path) -> io::Result<bool> {
        is_only(path, User::current()?.sid())
    }

    /// Whether `path`'s access list is what `restrict` makes: one entry, allowing `user`
    /// everything, and nothing inherited.
    fn is_only(path: &Path, user: PSID) -> io::Result<bool> {
        let name = wide(path);
        let mut dacl: *mut ACL = null_mut();
        let mut found: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: a NUL-ended name, and places for the ACL and the descriptor that holds it.
        let e = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut found,
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        // SAFETY: `dacl` (null for one that lets everyone in) is inside `found`, which is freed
        // once, after.
        let only = unsafe {
            let (mut control, mut revision) = (0u16, 0u32);
            let mut ace: *mut core::ffi::c_void = null_mut();
            let only = GetSecurityDescriptorControl(found, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0
                && !dacl.is_null()
                && (*dacl).AceCount == 1
                && GetAce(dacl, 0, &mut ace) != 0
                && {
                    let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    ace.Header.AceType == ACCESS_ALLOWED
                        && ace.Mask == FILE_ALL_ACCESS
                        && EqualSid((&raw const ace.SidStart).cast_mut().cast(), user) != 0
                };
            LocalFree(found);
            only
        };
        Ok(only)
    }

    pub fn user_sid() -> io::Result<String> {
        let user = User::current()?;
        let mut text = null_mut();
        // SAFETY: a valid SID, and a place for the string, freed once.
        unsafe {
            if ConvertSidToStringSidW(user.sid(), &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Ok(sid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-private-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_is_the_users_alone_from_the_start_and_when_written_again() {
        let dir = folder("file");
        let path = dir.join("secret");
        let mut file = create_new(&path).unwrap();
        assert!(
            is_private(&path).unwrap(),
            "private before it holds anything"
        );
        file.write_all(b"one").unwrap();
        drop(file);
        assert_eq!(
            create_new(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        write(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(is_private(&path).unwrap(), "and once it's replaced");
        // A file made beside it the ordinary way isn't, until it's made so.
        let open = dir.join("open");
        std::fs::write(&open, "x").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
        assert!(!is_private(&open).unwrap());
        restrict(&open).unwrap();
        assert!(is_private(&open).unwrap());
        // Nothing of the writes is left beside them.
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["open", "secret"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_folder_is_the_users_alone_whether_it_was_there_or_not() {
        let dir = folder("dir");
        let made = dir.join("a").join("b");
        create_dir(&made).unwrap();
        assert!(is_private(&made).unwrap());
        let there = dir.join("there");
        std::fs::create_dir(&there).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&there, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        assert!(!is_private(&there).unwrap());
        create_dir(&there).unwrap();
        assert!(is_private(&there).unwrap());
        // A secret written in it is private too.
        write(&there.join("token"), b"t").unwrap();
        assert!(is_private(&there.join("token")).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn the_user_is_named_as_windows_writes_a_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
