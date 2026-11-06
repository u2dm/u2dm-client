use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Action, Dialog, Encryption, EncryptionOutcome, Step};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::EncryptionStep;
use crate::domain::encryption::{KeyImport, KeyPassphrase};
use crate::ports::matrix::EncryptionPort;

const KEY_EXPORT_FILENAME: &str = "u2dm-room-keys.txt";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::app) enum KeyFile {
    Export,
    Import,
}

impl Encryption {
    pub(in crate::app) fn start_key_export(&mut self) {
        self.choose_file(KeyFile::Export);
    }

    pub(in crate::app) fn start_key_import(&mut self) {
        self.choose_file(KeyFile::Import);
    }

    pub(in crate::app) fn export_keys(
        &mut self,
        port: Arc<dyn EncryptionPort>,
        passphrase: KeyPassphrase,
    ) {
        let Some(Dialog {
            step: Step::Exporting(path),
            ..
        }) = self.dialog.as_ref()
        else {
            tracing::debug!("dropping a passphrase typed into a closed export");
            return;
        };
        if passphrase.expose().is_empty() {
            return;
        }
        let path = path.clone();
        self.act(Action::Export, move || async move {
            match port.export_room_keys(&path, &passphrase).await {
                Ok(count) => {
                    tracing::info!(count, "exported the room keys");
                    EncryptionOutcome::Exported(Some(count))
                }
                Err(e) => {
                    tracing::warn!("could not export the room keys: {e}");
                    EncryptionOutcome::Exported(None)
                }
            }
        });
    }

    pub(in crate::app) fn import_keys(
        &mut self,
        port: Arc<dyn EncryptionPort>,
        passphrase: KeyPassphrase,
    ) {
        let Some(Dialog {
            step: Step::Importing(path),
            ..
        }) = self.dialog.as_ref()
        else {
            tracing::debug!("dropping a passphrase typed into a closed import");
            return;
        };
        if passphrase.expose().is_empty() {
            return;
        }
        let path = path.clone();
        self.act(Action::Import, move || async move {
            match port.import_room_keys(&path, &passphrase).await {
                Ok(import) => {
                    tracing::info!(?import, "tried to import room keys");
                    EncryptionOutcome::Imported(Some(import))
                }
                Err(e) => {
                    tracing::warn!("could not import room keys: {e}");
                    EncryptionOutcome::Imported(None)
                }
            }
        });
    }

    fn choose_file(&mut self, file: KeyFile) {
        if self.dialog.is_none() || self.pending.is_some() {
            return;
        }
        let files = Arc::clone(&self.files);
        self.act(Action::ChooseFile, move || async move {
            let chosen = match file {
                KeyFile::Export => files.choose_key_export(KEY_EXPORT_FILENAME).await,
                KeyFile::Import => files.pick_key_file().await,
            };
            match chosen {
                Ok(path) => EncryptionOutcome::FileChosen(file, path),
                Err(e) => {
                    tracing::warn!(?file, "the file chooser failed: {e}");
                    EncryptionOutcome::Failed(match file {
                        KeyFile::Export => UserMessageKind::KeyExportFailed,
                        KeyFile::Import => UserMessageKind::KeyImportFailed,
                    })
                }
            }
        });
    }

    pub(super) fn file_chosen(&mut self, file: KeyFile, path: Option<PathBuf>) {
        let Some(path) = path else {
            tracing::debug!(?file, "no file chosen");
            return;
        };
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.error = UserMessage::default();
        dialog.step = match file {
            KeyFile::Export => Step::Exporting(path),
            KeyFile::Import => Step::Importing(path),
        };
    }

    pub(super) fn exported(&mut self, count: Option<usize>) {
        let Some(count) = count else {
            self.report(UserMessageKind::KeyExportFailed);
            return;
        };
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let file = match &dialog.step {
            Step::Exporting(path) => file_label(path),
            Step::Shown(_) | Step::Authenticating { .. } | Step::Importing(_) => String::new(),
        };
        *dialog = Dialog::at(EncryptionStep::KeysExported { count, file });
    }

    pub(super) fn imported(&mut self, import: Option<KeyImport>) {
        match import {
            Some(KeyImport::Imported { imported, total }) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    *dialog = Dialog::at(EncryptionStep::KeysImported { imported, total });
                }
            }
            Some(KeyImport::WrongPassphrase) => self.report(UserMessageKind::KeyPassphraseWrong),
            Some(KeyImport::NotAKeyFile) => self.report(UserMessageKind::KeyFileUnreadable),
            None => self.report(UserMessageKind::KeyImportFailed),
        }
    }
}

pub(super) fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}
