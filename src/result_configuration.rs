// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Shared conversions of parser/path failures into configuration diagnostics.
use std::fmt::Display;

use crate::dependency_error::DependencyError;

/// Retains configuration failure categories and caller-specific error context.
pub(crate) trait ResultConfiguration<T> {
    /// Preserves the original error's display text.
    fn configuration(self) -> Result<T, DependencyError>;
    /// Prefixes the original error with the supplied label and a colon.
    fn configuration_context(self, context: impl Display) -> Result<T, DependencyError>;
    /// Uses the supplied diagnostic when the original error text is irrelevant.
    fn configuration_message(self, message: impl Display) -> Result<T, DependencyError>;
}
impl<T, E: Display> ResultConfiguration<T> for Result<T, E> {
    fn configuration(self) -> Result<T, DependencyError> {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(DependencyError::Configuration(error.to_string())),
        }
    }
    fn configuration_context(self, context: impl Display) -> Result<T, DependencyError> {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(DependencyError::Configuration(format!("{context}: {error}"))),
        }
    }
    fn configuration_message(self, message: impl Display) -> Result<T, DependencyError> {
        match self {
            Ok(value) => Ok(value),
            Err(_) => Err(DependencyError::Configuration(message.to_string())),
        }
    }
}
