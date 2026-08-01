use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::workflow::WorkspaceAccess;

use super::{
    artifacts::ensure_private_directory, cancellation::CROSS_PROCESS_CANCEL_POLL, RuntimeError,
};

type LocalGate = tokio::sync::RwLock<()>;

fn local_gates() -> &'static Mutex<BTreeMap<PathBuf, Weak<LocalGate>>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<LocalGate>>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Clone)]
pub(crate) struct CheckoutGate {
    local: Arc<LocalGate>,
    lock_path: Arc<PathBuf>,
    _lock_anchor: Arc<File>,
}

impl CheckoutGate {
    pub(crate) fn new(rho_home: &Path, workspace: &Path) -> Result<Self, RuntimeError> {
        let workspace = workspace.canonicalize()?;
        let local = {
            let mut gates = local_gates().lock().expect("checkout gate map lock");
            gates
                .get(&workspace)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let gate = Arc::new(LocalGate::new(()));
                    gates.insert(workspace.clone(), Arc::downgrade(&gate));
                    gate
                })
        };
        let locks = rho_home.join("workflows").join("checkout-locks");
        ensure_private_directory(&locks)?;
        let key = format!(
            "{:x}",
            Sha256::digest(workspace.to_string_lossy().as_bytes())
        );
        let lock_path = locks.join(format!("{key}.lock"));
        let lock_file = open_lock_no_follow(&lock_path)?;
        Ok(Self {
            local,
            lock_path: Arc::new(lock_path),
            _lock_anchor: Arc::new(lock_file),
        })
    }

    pub(crate) async fn acquire(
        &self,
        access: WorkspaceAccess,
        cancellation: &rho_sdk::CancellationToken,
        wait_limit_seconds: u64,
    ) -> Result<CheckoutPermit, RuntimeError> {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(wait_limit_seconds);
        let local_wait = async {
            match access {
                WorkspaceAccess::ReadOnly => LocalPermit::Read {
                    _guard: self.local.clone().read_owned().await,
                },
                WorkspaceAccess::Mutating => LocalPermit::Write {
                    _guard: self.local.clone().write_owned().await,
                },
            }
        };
        let local = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
            () = tokio::time::sleep_until(deadline) => {
                return Err(RuntimeError::CheckoutLockTimeout { wait_limit_seconds });
            }
            local = local_wait => local,
        };
        let file = lock_file(
            &self.lock_path,
            access,
            cancellation,
            deadline,
            wait_limit_seconds,
        )
        .await?;
        Ok(CheckoutPermit {
            _local: local,
            file,
        })
    }
}

pub(crate) struct CheckoutPermit {
    _local: LocalPermit,
    file: File,
}

enum LocalPermit {
    Read {
        _guard: tokio::sync::OwnedRwLockReadGuard<()>,
    },
    Write {
        _guard: tokio::sync::OwnedRwLockWriteGuard<()>,
    },
}

impl Drop for CheckoutPermit {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

async fn lock_file(
    path: &Path,
    access: WorkspaceAccess,
    cancellation: &rho_sdk::CancellationToken,
    deadline: tokio::time::Instant,
    wait_limit_seconds: u64,
) -> Result<File, RuntimeError> {
    let file = open_lock_no_follow(path)?;
    if !file.metadata()?.is_file() {
        return Err(RuntimeError::Data(
            "checkout lock descriptor is not a regular file".to_owned(),
        ));
    }
    loop {
        if cancellation.is_cancelled() {
            return Err(RuntimeError::Cancelled);
        }
        let result = match access {
            WorkspaceAccess::ReadOnly => FileExt::try_lock_shared(&file),
            WorkspaceAccess::Mutating => FileExt::try_lock_exclusive(&file),
        };
        match result {
            Ok(()) if cancellation.is_cancelled() => {
                let _ = FileExt::unlock(&file);
                return Err(RuntimeError::Cancelled);
            }
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(RuntimeError::CheckoutLockTimeout { wait_limit_seconds });
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
            () = tokio::time::sleep_until((now + CROSS_PROCESS_CANCEL_POLL).min(deadline)) => {}
        }
    }
}

fn open_lock_no_follow(path: &Path) -> Result<File, RuntimeError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Receipt: FILE_FLAG_OPEN_REPARSE_POINT from the Windows file API.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(RuntimeError::Data(format!(
            "checkout lock '{}' is not a regular file",
            path.display()
        )));
    }
    Ok(file)
}
