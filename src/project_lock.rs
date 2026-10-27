// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Shared cancellable advisory locks for managed project outputs.
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use fs2::FileExt;

use crate::dependency_error::DependencyError;
use crate::option_configuration::OptionConfiguration;
use crate::runner::is_cancelled;

/// Waits for an exclusive lock while honoring cancellation and the stage
/// budget.
pub(crate) fn acquire(path: &Path, timeout_seconds: u64) -> Result<File, DependencyError> {
    let parent = path.parent().required_configuration("lock path has no parent")?;
    fs::create_dir_all(parent)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if is_cancelled() {
            return Err(DependencyError::Cancelled);
        }
        if started.elapsed() > Duration::from_secs(timeout_seconds) {
            return Err(DependencyError::Gate(format!(
                "timed out waiting for project lock: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}
