//! Core library for Mica.
//!
//! Domain modules deliberately do not depend on terminal UI types. The UI
//! dispatches commands and renders immutable state snapshots.

pub mod app;
pub mod buffer;
pub mod cli;
pub mod command;
pub mod config;
pub mod diagnostics;
pub mod editor;
pub mod git;
pub mod lsp;
pub mod search;
pub mod session;
pub mod terminal;
pub mod ui;
pub mod workspace;
