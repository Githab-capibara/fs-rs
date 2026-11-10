//! The dashboard of `fs-rs`.
//!
//! The crate is split into a library and a thin binary:
//!
//! * [`app`] builds the window: the repository strip, the table of the
//!   upstream commits, the model picker, the terminal and the log of the
//!   pipeline,
//! * [`state`] holds the data of the window and the pure functions that
//!   belong to it,
//! * [`terminal`] runs the OpenCode CLI in a pseudo terminal,
//! * [`worker`] does the slow work in the background.
//!
//! The binary `fsrs` only reads the configuration, opens the database and
//! starts the window.

#![warn(missing_docs)]

pub mod app;
pub mod state;
pub mod terminal;
pub mod worker;
