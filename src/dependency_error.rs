// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Stable failure categories for embedding the dependency command.
use thiserror::Error;

/// A dependency failure with a stable process exit category.
#[derive(Debug, Error)]
pub enum DependencyError {
    /// Invalid configuration, protocol, unsupported tool version or unsafe
    /// path.
    #[error("{0}")]
    Configuration(String),
    /// Conan resolution, installation, or dependency policy rejected the
    /// request.
    #[error("{0}")]
    Gate(String),
    /// Filesystem or process launch failed.
    #[error("{0}")]
    Infrastructure(#[from] std::io::Error),
    /// The user interrupted an external command.
    #[error("operation cancelled")]
    Cancelled,
}
impl DependencyError {
    /// Returns the common cpp-infra exit code for this failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Configuration(_) => 2,
            Self::Gate(_) => 1,
            Self::Infrastructure(_) => 3,
            Self::Cancelled => 130,
        }
    }
}
