use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path},
};

use sha2::{Digest as _, Sha256};

use super::{
    Digest, ExecutableIdentity, FrozenPathIdentity, FrozenPathKind, WorkflowError, WorkflowResult,
};

pub(crate) struct VerifiedPath {
    pub(crate) file: File,
    pub(crate) identity: FrozenPathIdentity,
}

pub(crate) struct VerifiedExecutable {
    pub(crate) executable: VerifiedPath,
    pub(crate) interpreter: Option<VerifiedPath>,
    pub(crate) interpreter_arguments: Vec<String>,
}

pub(crate) fn open_file_beneath(root: &Path, relative: &Path) -> WorkflowResult<File> {
    open_beneath(root, relative, FrozenPathKind::File)
}

#[cfg(unix)]
pub(crate) fn write_file_beneath(root: &Path, relative: &Path, bytes: &[u8]) -> WorkflowResult<()> {
    use std::ffi::CString;
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};

    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    ensure_directory_beneath(root, parent)?;
    let directory = open_beneath(root, parent, FrozenPathKind::Directory)?;
    let Some(Component::Normal(file_name)) = relative.components().next_back() else {
        return Err(identity_drift(relative, "invalid file name"));
    };
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| identity_drift(relative, "NUL in file name"))?;
    let temporary_name = CString::new(format!(".rho-tmp-{}", uuid::Uuid::new_v4()))
        .expect("UUID temporary name has no NUL");
    let directory_fd = std::os::fd::AsRawFd::as_raw_fd(&directory);
    // SAFETY: the directory descriptor and C string are valid; the new descriptor is owned below.
    let fd = unsafe {
        libc::openat(
            directory_fd,
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: openat returned a new owned descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        // SAFETY: both names are valid and remain in the held parent directory.
        if unsafe {
            libc::renameat(
                directory_fd,
                temporary_name.as_ptr(),
                directory_fd,
                file_name.as_ptr(),
            )
        } == -1
        {
            return Err(std::io::Error::last_os_error());
        }
        directory.sync_all()
    })();
    if result.is_err() {
        // SAFETY: the temporary name and held parent descriptor are valid.
        unsafe { libc::unlinkat(directory_fd, temporary_name.as_ptr(), 0) };
    }
    result.map_err(WorkflowError::Io)
}

#[cfg(unix)]
pub(crate) fn ensure_directory_beneath(root: &Path, relative: &Path) -> WorkflowResult<()> {
    use std::ffi::CString;
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};

    let root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| identity_drift(root, "NUL in root"))?;
    // SAFETY: root is a valid C string and the returned descriptor is owned below.
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: open returned a new owned descriptor.
    let mut current = unsafe { File::from_raw_fd(root_fd) };
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(identity_drift(relative, "path traversal is not allowed"));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| identity_drift(relative, "NUL"))?;
        let current_fd = std::os::fd::AsRawFd::as_raw_fd(&current);
        // SAFETY: the held directory descriptor and C string are valid.
        if unsafe { libc::mkdirat(current_fd, name.as_ptr(), 0o700) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        // SAFETY: the held directory descriptor and C string are valid.
        let fd = unsafe {
            libc::openat(
                current_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a new owned descriptor.
        current = unsafe { File::from_raw_fd(fd) };
        // SAFETY: current holds a valid directory descriptor.
        if unsafe { libc::fchmod(std::os::fd::AsRawFd::as_raw_fd(&current), 0o700) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_directory_beneath(root: &Path, relative: &Path) -> WorkflowResult<()> {
    let mut current = root.to_owned();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(identity_drift(relative, "path traversal is not allowed"));
        };
        current.push(component);
        match std::fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(&current)?;
        #[cfg(windows)]
        let reparse_point = {
            use std::os::windows::fs::MetadataExt as _;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse_point = metadata.file_type().is_symlink();
        if !metadata.is_dir() || reparse_point {
            return Err(WorkflowError::SourceSymlink { path: current });
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_file_beneath(root: &Path, relative: &Path, bytes: &[u8]) -> WorkflowResult<()> {
    let parent = relative
        .parent()
        .ok_or_else(|| identity_drift(relative, "file has no parent"))?;
    ensure_directory_beneath(root, parent)?;
    let path = root.join(relative);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    let reparse_point = {
        use std::os::windows::fs::MetadataExt as _;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse_point = metadata.file_type().is_symlink();
    if reparse_point {
        return Err(WorkflowError::SourceSymlink { path });
    }
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn read_source_beneath(
    root: &Path,
    relative: &Path,
    budget: &super::Budget,
    retained: u64,
) -> WorkflowResult<String> {
    let mut file = open_beneath(root, relative, FrozenPathKind::File)?;
    let measured = retained.saturating_add(file.metadata()?.len());
    budget.check(measured)?;
    let remaining = budget.limit.saturating_sub(retained);
    let mut bytes = Vec::with_capacity(usize::try_from(remaining.min(8 * 1024)).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(remaining.saturating_add(1))
        .read_to_end(&mut bytes)?;
    budget.check(retained.saturating_add(bytes.len() as u64))?;
    String::from_utf8(bytes).map_err(|error| WorkflowError::Starlark(error.to_string()))
}

pub(crate) fn freeze_executable_identity(path: &Path) -> WorkflowResult<ExecutableIdentity> {
    let executable = inspect_absolute(path, FrozenPathKind::File, true)?;
    let interpreter = shebang_interpreter(&executable.file)?;
    let interpreter_identity = interpreter
        .as_ref()
        .map(|spec| {
            let verified = inspect_absolute(&spec.path, FrozenPathKind::File, true)?;
            if shebang_interpreter(&verified.file)?.is_some() {
                return Err(WorkflowError::Corrupt {
                    path: spec.path.clone(),
                    reason: "nested script interpreters are not supported for frozen execution"
                        .to_owned(),
                });
            }
            Ok(verified.identity)
        })
        .transpose()?;
    Ok(ExecutableIdentity {
        file: executable.identity,
        interpreter: interpreter_identity,
        interpreter_arguments: interpreter.map_or_else(Vec::new, |spec| spec.arguments),
    })
}

pub(crate) fn verify_executable_identity(
    expected: &ExecutableIdentity,
) -> WorkflowResult<VerifiedExecutable> {
    let executable = verify_path_identity(&expected.file)?;
    let measured_interpreter = shebang_interpreter(&executable.file)?;
    let interpreter = match (&expected.interpreter, measured_interpreter) {
        (None, None) => None,
        (Some(expected_interpreter), Some(spec))
            if Path::new(&expected_interpreter.canonical_path) == spec.path
                && spec.arguments == expected.interpreter_arguments =>
        {
            Some(verify_path_identity(expected_interpreter)?)
        }
        _ => {
            return Err(identity_drift(
                Path::new(&expected.file.canonical_path),
                "script interpreter changed",
            ))
        }
    };
    Ok(VerifiedExecutable {
        executable,
        interpreter,
        interpreter_arguments: expected.interpreter_arguments.clone(),
    })
}

pub(crate) fn freeze_directory_identity(path: &Path) -> WorkflowResult<FrozenPathIdentity> {
    Ok(inspect_absolute(path, FrozenPathKind::Directory, false)?.identity)
}

pub(crate) fn verify_directory_identity(
    expected: &FrozenPathIdentity,
) -> WorkflowResult<VerifiedPath> {
    verify_path_identity(expected)
}

pub(crate) fn verified_handle_path(
    file: &File,
    _fallback: &Path,
) -> WorkflowResult<std::path::PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        // The child must retain script and directory descriptors until exec resolves /proc.
        // SAFETY: fcntl only changes flags on this valid owned descriptor.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(std::path::PathBuf::from(format!("/proc/self/fd/{fd}")))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = file;
        Ok(_fallback.to_owned())
    }
}

fn verify_path_identity(expected: &FrozenPathIdentity) -> WorkflowResult<VerifiedPath> {
    let measured = inspect_absolute(
        Path::new(&expected.canonical_path),
        expected.kind,
        expected.content_digest.is_some(),
    )?;
    if &measured.identity != expected {
        return Err(identity_drift(
            Path::new(&expected.canonical_path),
            &format!("expected {expected:?}, measured {:?}", measured.identity),
        ));
    }
    Ok(measured)
}

fn inspect_absolute(path: &Path, kind: FrozenPathKind, hash: bool) -> WorkflowResult<VerifiedPath> {
    if !path.is_absolute() {
        return Err(WorkflowError::Corrupt {
            path: path.to_owned(),
            reason: "frozen identity path is not absolute".to_owned(),
        });
    }
    let canonical = path.canonicalize()?;
    let relative = canonical
        .strip_prefix(canonical_root(&canonical))
        .map_err(|_| identity_drift(path, "cannot select a filesystem root"))?;
    let mut file = open_beneath(canonical_root(&canonical), relative, kind)?;
    let metadata = file.metadata()?;
    let content_digest = if hash {
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        file.seek(SeekFrom::Start(0))?;
        Some(Digest(format!("sha256:{:x}", hasher.finalize())))
    } else {
        None
    };
    let platform_id = platform_id(&file, &metadata);
    #[cfg(windows)]
    if platform_id.is_none() {
        return Err(identity_drift(path, "Windows file identity is unavailable"));
    }
    let identity = FrozenPathIdentity {
        canonical_path: crate::paths::display(&canonical),
        kind,
        byte_len: if kind == FrozenPathKind::File {
            metadata.len()
        } else {
            0
        },
        content_digest,
        platform_id,
    };
    Ok(VerifiedPath { file, identity })
}

fn canonical_root(path: &Path) -> &Path {
    path.ancestors().last().expect("absolute path has a root")
}

#[cfg(unix)]
fn open_beneath(root: &Path, relative: &Path, kind: FrozenPathKind) -> WorkflowResult<File> {
    use std::ffi::CString;
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};

    let root =
        CString::new(root.as_os_str().as_bytes()).map_err(|_| identity_drift(root, "NUL"))?;
    // SAFETY: root is a valid C string and the returned descriptor is owned below.
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: open returned a new owned descriptor.
    let mut current = unsafe { File::from_raw_fd(root_fd) };
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return match kind {
            FrozenPathKind::Directory => Ok(current),
            FrozenPathKind::File => Err(identity_drift(relative, "expected a file")),
        };
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(identity_drift(relative, "path traversal is not allowed"));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| identity_drift(relative, "NUL"))?;
        let last = index + 1 == components.len();
        let directory = !last || kind == FrozenPathKind::Directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if directory { libc::O_DIRECTORY } else { 0 };
        // SAFETY: current and name stay valid for the call; openat returns a new descriptor.
        let fd = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&current),
                name.as_ptr(),
                flags,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openat returned a new owned descriptor.
        current = unsafe { File::from_raw_fd(fd) };
    }
    Ok(current)
}

#[cfg(windows)]
fn open_beneath(root: &Path, relative: &Path, kind: FrozenPathKind) -> WorkflowResult<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    const FILE_ATTRIBUTE_REPARSE_POINT_VALUE: u32 = 0x400;

    let path = root.join(relative);
    let mut current = root.to_owned();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(identity_drift(&path, "path traversal is not allowed"));
        };
        current.push(component);
        if std::fs::symlink_metadata(&current)?.file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT_VALUE
            != 0
        {
            return Err(WorkflowError::SourceSymlink { path: current });
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(&path)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT_VALUE != 0 {
        return Err(WorkflowError::SourceSymlink { path });
    }
    if (kind == FrozenPathKind::File && !metadata.is_file())
        || (kind == FrozenPathKind::Directory && !metadata.is_dir())
    {
        return Err(identity_drift(&path, "path kind changed"));
    }
    Ok(file)
}

#[cfg(all(not(unix), not(windows)))]
fn open_beneath(root: &Path, relative: &Path, kind: FrozenPathKind) -> WorkflowResult<File> {
    let path = root.join(relative);
    for component in relative.components() {
        let Component::Normal(_) = component else {
            return Err(identity_drift(&path, "path traversal is not allowed"));
        };
    }
    let file = File::open(&path)?;
    let metadata = file.metadata()?;
    if (kind == FrozenPathKind::File && !metadata.is_file())
        || (kind == FrozenPathKind::Directory && !metadata.is_dir())
    {
        return Err(identity_drift(&path, "path kind changed"));
    }
    Ok(file)
}

#[cfg(unix)]
fn platform_id(_file: &File, metadata: &Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn platform_id(file: &File, _metadata: &Metadata) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the file owns a valid handle and information points to writable storage.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return None;
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Some(format!(
        "windows:{}:{}",
        information.dwVolumeSerialNumber, index
    ))
}

#[cfg(all(not(unix), not(windows)))]
fn platform_id(_file: &File, _metadata: &Metadata) -> Option<String> {
    None
}

struct Shebang {
    path: std::path::PathBuf,
    arguments: Vec<String>,
}

fn shebang_interpreter(file: &File) -> WorkflowResult<Option<Shebang>> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    // Receipt: Linux BINPRM_BUF_SIZE is 256 bytes; longer shebangs are not portable.
    let mut bytes = [0_u8; 256];
    let read = file.read(&mut bytes)?;
    if !bytes[..read].starts_with(b"#!") {
        return Ok(None);
    }
    let line = bytes[2..read]
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let line = std::str::from_utf8(line)
        .map_err(|_| identity_drift(Path::new("<script>"), "shebang is not UTF-8"))?;
    let mut words = line.split_ascii_whitespace();
    let interpreter = words
        .next()
        .ok_or_else(|| identity_drift(Path::new("<script>"), "shebang has no interpreter"))?;
    let mut path = Path::new(interpreter).to_owned();
    let mut arguments = words.map(str::to_owned).collect::<Vec<_>>();
    if path.file_name().is_some_and(|name| name == "env") {
        let target = arguments.first().ok_or_else(|| {
            identity_drift(Path::new("<script>"), "env shebang has no interpreter")
        })?;
        if target.starts_with('-') {
            return Err(identity_drift(
                &path,
                "env shebang options are not supported for frozen execution",
            ));
        }
        path = crate::executable::find_on_path(target).ok_or_else(|| {
            identity_drift(
                Path::new(target),
                "env shebang interpreter was not found on PATH",
            )
        })?;
        arguments.remove(0);
    }
    if !path.is_absolute() {
        return Err(identity_drift(
            &path,
            "shebang must name an absolute interpreter",
        ));
    }
    Ok(Some(Shebang { path, arguments }))
}

fn identity_drift(path: &Path, reason: &str) -> WorkflowError {
    WorkflowError::Corrupt {
        path: path.to_owned(),
        reason: format!("frozen filesystem identity drift: {reason}"),
    }
}
