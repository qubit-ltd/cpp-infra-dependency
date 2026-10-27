// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Public command entry adapter.
use crate::cli;

/// Runs the CLI, executes the selected workflow, and writes a common JSON
/// report.
///
/// External commands may execute the selected Conan recipe. Returns the
/// cpp-infra exit category: 0 success, 1 gate failure, 2 configuration, 3 I/O,
/// 130 cancellation.
pub fn main_entry() -> i32 {
    cli::entry()
}
