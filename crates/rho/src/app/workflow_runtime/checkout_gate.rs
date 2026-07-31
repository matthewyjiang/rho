use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::workflow::WorkspaceAccess;

use super::{artifacts::ensure_private_directory, RuntimeError};

type LocalGate = tokio::sync::RwLock<()>;

fn local_gates() -> &'static Mutex<BTreeMap<PathBuf, Weak<LocalGate>>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<LocalGate>>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Clone)]
pub(crate) struct CheckoutGate {
    local: Arc<LocalGate>,
    lock_path: PathBuf,
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
        if !lock_path.exists() {
            fs::write(&lock_path, [])?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok(Self { local, lock_path })
    }

    pub(crate) async fn acquire(
        &self,
        access: WorkspaceAccess,
    ) -> Result<CheckoutPermit, RuntimeError> {
        let local = match access {
            WorkspaceAccess::ReadOnly => LocalPermit::Read {
                _guard: self.local.clone().read_owned().await,
            },
            WorkspaceAccess::Mutating => LocalPermit::Write {
                _guard: self.local.clone().write_owned().await,
            },
        };
        let path = self.lock_path.clone();
        let file = tokio::task::spawn_blocking(move || lock_file(&path, access))
            .await
            .map_err(|error| {
                RuntimeError::Executor(format!("checkout lock task failed: {error}"))
            })??;
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

fn lock_file(path: &Path, access: WorkspaceAccess) -> Result<File, RuntimeError> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    match access {
        WorkspaceAccess::ReadOnly => file.lock_shared()?,
        WorkspaceAccess::Mutating => file.lock_exclusive()?,
    }
    Ok(file)
}
