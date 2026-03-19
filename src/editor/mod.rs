// Editor core API — used in Stories 06 (file editing) and 07 (Vi mode).
#![allow(dead_code)]

//! Editor core — pure state machine, no I/O, no rendering.
//!
//! Architecture:
//!   `buffer`  — `Buffer` trait + `InMemoryBuffer` (Phase 1 concrete impl)
//!   `state`   — `EditorState`, `Pos`, `Selection`, `Mode`
//!   `command` — `EditorCommand` enum
//!   `apply`   — `apply(cmd, state, buf) -> (state, SideEffect)`
//!
//! The split is intentional: state can be read without a mutable buffer
//! reference, and commands can be constructed anywhere without importing buffer
//! internals.

pub mod apply;
pub mod buffer;
pub mod command;
pub mod state;

