use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::adapters::private_fs;
use crate::ports::matrix::LocalDataOwnership;

const LOCK_FILE: &str = "instance.lock";

pub(crate) enum InstanceClaim {
    Sole {
        lock: PathBuf,
        held_while_open: File,
    },
    AnotherInstance {
        lock: PathBuf,
    },
    Undetermined {
        reason: String,
    },
}

impl InstanceClaim {
    pub(crate) fn take(data_dir: &Path) -> Self {
        let lock = data_dir.join(LOCK_FILE);
        let file = match open_lock_file(data_dir, &lock) {
            Ok(file) => file,
            Err(e) => return Self::undetermined(&lock, &e),
        };
        match file.try_lock() {
            Ok(()) => Self::sole(lock, file),
            Err(TryLockError::WouldBlock) => Self::another_instance(lock),
            Err(TryLockError::Error(e)) => Self::undetermined(&lock, &e),
        }
    }

    pub(crate) fn ownership(&self) -> LocalDataOwnership {
        match self {
            Self::Sole { .. } => LocalDataOwnership::Exclusive,
            Self::AnotherInstance { lock } => LocalDataOwnership::AnotherInstance {
                lock: lock.display().to_string(),
            },
            Self::Undetermined { reason } => LocalDataOwnership::Undetermined {
                reason: reason.clone(),
            },
        }
    }

    fn sole(lock: PathBuf, file: File) -> Self {
        tracing::info!(
            path = %lock.display(),
            "this instance holds the local data for as long as it runs"
        );
        Self::Sole {
            lock,
            held_while_open: file,
        }
    }

    fn another_instance(lock: PathBuf) -> Self {
        tracing::error!(
            path = %lock.display(),
            "another U2DM instance already holds the local data"
        );
        Self::AnotherInstance { lock }
    }

    fn undetermined(lock: &Path, error: &io::Error) -> Self {
        tracing::error!(
            path = %lock.display(),
            "this instance could not claim the local data: {error}"
        );
        Self::Undetermined {
            reason: format!("{} ({error})", lock.display()),
        }
    }
}

impl Drop for InstanceClaim {
    fn drop(&mut self) {
        let Self::Sole {
            lock,
            held_while_open,
        } = self
        else {
            return;
        };
        match held_while_open.unlock() {
            Ok(()) => tracing::info!(
                path = %lock.display(),
                "released this instance's hold on the local data"
            ),
            Err(e) => tracing::debug!(
                path = %lock.display(),
                "could not release this instance's hold on the local data: {e}"
            ),
        }
    }
}

fn open_lock_file(data_dir: &Path, lock: &Path) -> io::Result<File> {
    private_fs::create_dir_blocking(data_dir)?;
    private_fs::create_or_open_private_blocking(lock)
}
