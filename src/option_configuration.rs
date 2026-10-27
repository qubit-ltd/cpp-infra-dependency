// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Required configuration values with one consistent typed failure boundary.
use std::fmt::Display;

use crate::dependency_error::DependencyError;

/// Converts an absent schema/configuration value into its existing diagnostic.
pub(crate) trait OptionConfiguration<T> {
    /// Returns the value, or renders the supplied diagnostic only when absent.
    fn required_configuration(self, message: impl Display) -> Result<T, DependencyError>;
}
impl<T> OptionConfiguration<T> for Option<T> {
    fn required_configuration(self, message: impl Display) -> Result<T, DependencyError> {
        match self {
            Some(value) => Ok(value),
            None => Err(DependencyError::Configuration(message.to_string())),
        }
    }
}
