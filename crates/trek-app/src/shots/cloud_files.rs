//! A sync provider of the capture build's own, for the live check of files that live only in
//! the cloud (`cloud-file`, Windows): its folder is a sync root as OneDrive's is, its files are
//! placeholders whose data isn't on the disk, and Windows asks it for a file's data when
//! something reads the file, as it asks OneDrive. It answers after `ANSWER_AFTER`, so a check
//! sees the reader waiting; `away` disconnects it, as OneDrive not running does, and `off`
//! leaves the folder an ordinary one again.

use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION,
    CF_CALLBACK_TYPE_FETCH_DATA, CF_CALLBACK_TYPE_NONE, CF_CONNECT_FLAG_NONE, CF_CONNECTION_KEY,
    CF_CREATE_FLAG_NONE, CF_FS_METADATA, CF_HARDLINK_POLICY_NONE, CF_HYDRATION_POLICY,
    CF_HYDRATION_POLICY_FULL, CF_HYDRATION_POLICY_MODIFIER_NONE, CF_INSYNC_POLICY_NONE,
    CF_OPERATION_INFO, CF_OPERATION_PARAMETERS, CF_OPERATION_PARAMETERS_0,
    CF_OPERATION_PARAMETERS_0_0, CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
    CF_OPERATION_TYPE_TRANSFER_DATA, CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
    CF_PLACEHOLDER_CREATE_FLAG_SUPERSEDE, CF_PLACEHOLDER_CREATE_INFO,
    CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY,
    CF_POPULATION_POLICY_ALWAYS_FULL, CF_POPULATION_POLICY_MODIFIER_NONE, CF_REGISTER_FLAG_NONE,
    CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CfConnectSyncRoot, CfCreatePlaceholders,
    CfDisconnectSyncRoot, CfExecute, CfRegisterSyncRoot, CfUnregisterSyncRoot,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO};
use windows_sys::core::GUID;

/// How long the provider takes to hand a file over: long enough for a check to see Trek waiting
/// on it, as it would on a slow connection.
const ANSWER_AFTER: Duration = Duration::from_millis(2500);

/// Times Windows asked the provider for a file's data.
static FETCHED: AtomicUsize = AtomicUsize::new(0);

/// The sync root, and the provider's connection to it while there is one.
static ROOT: Mutex<Option<(PathBuf, Option<CF_CONNECTION_KEY>)>> = Mutex::new(None);

/// What Windows calls: for a file's data, and nothing else (the files are all made up front).
static CALLBACKS: [CF_CALLBACK_REGISTRATION; 2] = [
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_FETCH_DATA,
        Callback: Some(fetch_data),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NONE,
        Callback: None,
    },
];

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

fn ok(result: i32, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        result >= 0,
        "{what}: {:#010x} ({})",
        result as u32,
        std::io::Error::from_raw_os_error(result)
    );
    Ok(())
}

/// `path` a file that lives only in the cloud, whose data is `text`: a placeholder in a sync
/// root of the provider's own (its folder, registered and connected the first time).
pub fn file(path: &Path, text: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!text.is_empty(), "cloud-file: the file needs some text");
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        anyhow::bail!("cloud-file: {} isn't a file's path", path.display());
    };
    std::fs::create_dir_all(folder)?;
    let mut root = ROOT.lock().unwrap_or_else(|e| e.into_inner());
    match root.as_ref() {
        Some((at, _)) if at != folder => anyhow::bail!(
            "cloud-file: one folder at a time ({} is already)",
            at.display()
        ),
        Some(_) => {}
        None => {
            register(folder)?;
            *root = Some((folder.to_path_buf(), None));
        }
    }
    if let Some((_, key @ None)) = root.as_mut() {
        let mut connection: CF_CONNECTION_KEY = 0;
        let at = wide(folder.as_os_str());
        // SAFETY: a NUL-terminated path and the callback table, which lives as long as the
        // program; Windows fills `connection`.
        let result = unsafe {
            CfConnectSyncRoot(
                at.as_ptr(),
                CALLBACKS.as_ptr(),
                std::ptr::null(),
                CF_CONNECT_FLAG_NONE,
                &mut connection,
            )
        };
        ok(result, "CfConnectSyncRoot")?;
        *key = Some(connection);
    }
    // Windows' file times: 100 ns since 1601.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_nanos() / 100) as i64 + 116_444_736_000_000_000);
    let name = wide(name);
    // The file's data is its identity, which Windows hands back with every request for it.
    let mut placeholder = CF_PLACEHOLDER_CREATE_INFO {
        RelativeFileName: name.as_ptr(),
        FsMetadata: CF_FS_METADATA {
            BasicInfo: FILE_BASIC_INFO {
                CreationTime: now,
                LastAccessTime: now,
                LastWriteTime: now,
                ChangeTime: now,
                FileAttributes: FILE_ATTRIBUTE_NORMAL,
            },
            FileSize: text.len() as i64,
        },
        FileIdentity: text.as_ptr().cast(),
        FileIdentityLength: text.len() as u32,
        Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC | CF_PLACEHOLDER_CREATE_FLAG_SUPERSEDE,
        Result: 0,
        CreateUsn: 0,
    };
    let at = wide(folder.as_os_str());
    let mut made = 0u32;
    // SAFETY: NUL-terminated paths and one placeholder whose pointers outlive the call.
    let result = unsafe {
        CfCreatePlaceholders(
            at.as_ptr(),
            &mut placeholder,
            1,
            CF_CREATE_FLAG_NONE,
            &mut made,
        )
    };
    ok(result, "CfCreatePlaceholders")?;
    ok(placeholder.Result, "the placeholder")
}

/// `folder` a sync root of the provider's own (a stale one from a run that ended early goes
/// first).
fn register(folder: &Path) -> anyhow::Result<()> {
    let at = wide(folder.as_os_str());
    // SAFETY: a NUL-terminated path; failing is fine (it wasn't one).
    unsafe { CfUnregisterSyncRoot(at.as_ptr()) };
    let name = wide("Trek live check".as_ref());
    let version = wide("1".as_ref());
    let identity = b"trek-live-check";
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: identity.as_ptr().cast(),
        SyncRootIdentityLength: identity.len() as u32,
        FileIdentity: std::ptr::null(),
        FileIdentityLength: 0,
        ProviderId: GUID::from_u128(0x6b1e_7a52_93c4_4d0f_8e2a_5f3c_9d81_b7e4),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_FULL,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    // SAFETY: the structures and the strings they point to outlive the call.
    let result =
        unsafe { CfRegisterSyncRoot(at.as_ptr(), &registration, &policies, CF_REGISTER_FLAG_NONE) };
    ok(result, "CfRegisterSyncRoot")
}

/// Windows wants a file's data: after `ANSWER_AFTER`, all of it (the file's identity).
unsafe extern "system" fn fetch_data(
    info: *const CF_CALLBACK_INFO,
    _parameters: *const CF_CALLBACK_PARAMETERS,
) {
    FETCHED.fetch_add(1, Ordering::Relaxed);
    std::thread::sleep(ANSWER_AFTER);
    // SAFETY: Windows' own, for the length of this call.
    let info = unsafe { &*info };
    let operation = CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_DATA,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        CorrelationVector: info.CorrelationVector.cast_const(),
        SyncStatus: std::ptr::null(),
        RequestKey: info.RequestKey,
    };
    let mut transfer = CF_OPERATION_PARAMETERS {
        ParamSize: (std::mem::offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32,
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            TransferData: CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: 0,
                Buffer: info.FileIdentity,
                Offset: 0,
                Length: i64::from(info.FileIdentityLength),
            },
        },
    };
    // SAFETY: the request's own keys, and a buffer (the identity) that outlives the call.
    unsafe { CfExecute(&operation, &mut transfer) };
}

/// Times Windows asked for a file's data, since the provider started.
pub fn fetched() -> usize {
    FETCHED.load(Ordering::Relaxed)
}

/// The provider stops answering, as OneDrive not running: reading a file it keeps now fails.
pub fn away() -> anyhow::Result<()> {
    let mut root = ROOT.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, key)) = root.as_mut() else {
        anyhow::bail!("cloud-file away: no files of its own yet");
    };
    if let Some(connection) = key.take() {
        // SAFETY: the connection `file` made.
        ok(
            unsafe { CfDisconnectSyncRoot(connection) },
            "CfDisconnectSyncRoot",
        )?;
    }
    Ok(())
}

/// The folder an ordinary one again: the provider gone, its sync root too.
pub fn off() -> anyhow::Result<()> {
    let Some((folder, key)) = ROOT.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return Ok(());
    };
    if let Some(connection) = key {
        // SAFETY: the connection `file` made.
        unsafe { CfDisconnectSyncRoot(connection) };
    }
    let at = wide(folder.as_os_str());
    // SAFETY: a NUL-terminated path.
    ok(
        unsafe { CfUnregisterSyncRoot(at.as_ptr()) },
        "CfUnregisterSyncRoot",
    )
}
