// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Conan 2 dependency governance with strict policy and auditable process
//! reports.
mod application;
mod baseline;
mod cli;
mod conan_adapter;
mod config;
pub mod dependency_error;
mod digest;
mod graph;
mod lock_update;
mod option_configuration;
mod prepare;
mod project_lock;
mod report;
mod result_configuration;
mod runner;
mod source_exclusions;
mod text_sync;

pub use application::main_entry;
