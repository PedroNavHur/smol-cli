use std::{fs, io::ErrorKind, path::PathBuf};

use anyhow::Result;

use crate::fsutil;

use super::state::{App, MessageKind};

#[derive(Clone)]
pub(super) struct PreparedEdit {
    pub(super) path: String,
    pub(super) abs_path: PathBuf,
    pub(super) diff: String,
    pub(super) rationale: Option<String>,
    pub(super) new_contents: String,
}

pub(super) struct ReviewState {
    pub(super) edits: Vec<PreparedEdit>,
    pub(super) index: usize,
    pub(super) backup_root: PathBuf,
}

impl ReviewState {
    pub(super) fn current_edit(&self) -> Option<&PreparedEdit> {
        self.edits.get(self.index)
    }
}

pub(super) fn apply_current(app: &mut App) -> Result<()> {
    let (edit, backup_root): (PreparedEdit, PathBuf) = match app
        .review
        .as_ref()
        .and_then(|r| r.current_edit().map(|e| (e.clone(), r.backup_root.clone())))
    {
        Some(tuple) => tuple,
        None => return Ok(()),
    };

    let backup_file = fsutil::backup_path(&backup_root, &edit.abs_path, &app.repo_root)?;
    fsutil::backup_and_write(&edit.abs_path, &edit.new_contents, &backup_file)?;
    app.add_message(
        MessageKind::Info,
        format!("Applied {} (backup: {})", edit.path, backup_file.display()),
    );
    app.last_backups.push(backup_file);
    advance_review(app);
    Ok(())
}

pub(super) fn skip_current(app: &mut App, reason: &str) {
    if let Some(review) = &app.review {
        if let Some(current) = review.current_edit() {
            app.add_message(MessageKind::Info, format!("{}: {}", reason, current.path));
        }
    }
    advance_review(app);
}

fn advance_review(app: &mut App) {
    if let Some(review) = &mut app.review {
        review.index += 1;
        if review.index >= review.edits.len() {
            app.review = None;
            app.add_message(MessageKind::Info, "Review complete.".into());
            app.caret_visible = true;
        }
    }
}

pub(super) fn undo_last(app: &mut App) {
    if let Some(backup) = app.last_backups.pop() {
        match super::state::target_from_backup(&app.repo_root, &backup) {
            Some(target) => {
                if backup.exists() {
                    match fs::copy(&backup, &target) {
                        Ok(_) => app.add_message(
                            MessageKind::Info,
                            format!("Reverted {}", target.display()),
                        ),
                        Err(err) => {
                            app.add_message(MessageKind::Error, format!("Undo failed: {err}"))
                        }
                    }
                } else {
                    match fs::remove_file(&target) {
                        Ok(_) => app.add_message(
                            MessageKind::Info,
                            format!("Removed {}", target.display()),
                        ),
                        Err(err) if err.kind() == ErrorKind::NotFound => app.add_message(
                            MessageKind::Info,
                            format!("Nothing to undo for {}", target.display()),
                        ),
                        Err(err) => {
                            app.add_message(MessageKind::Error, format!("Undo failed: {err}"))
                        }
                    }
                }
            }
            None => app.add_message(
                MessageKind::Warn,
                format!("Could not determine target for {}", backup.display()),
            ),
        }
    } else {
        app.add_message(MessageKind::Info, "Nothing to undo.".into());
    }
    app.caret_visible = true;
}
