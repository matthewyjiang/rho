//! Deterministic PTY harness for Rho's interactive TUI.
//!
//! Layers:
//! - [`pty`]: spawn a binary in a pseudo-terminal, inject input, resize, drain
//! - [`screen`]: reconstruct the visible terminal via a VT parser
//! - [`harness`]: high-level waits, assertions, and failure artifacts
//! - [`scenario`]: named scripted flows over the fixture matrix
//!
//! This crate is test support only. It is not linked into the production `rho`
//! binary.

#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

pub mod artifacts;
pub mod env;
pub mod harness;
pub mod keys;
pub mod pty;
pub mod scenario;
pub mod scenarios;
pub mod screen;
pub mod timing;

pub use artifacts::{ArtifactBundle, ArtifactWriter};
pub use env::{default_clean_env, HostProfile, IsolatedHome, RhoLaunchPlan};
pub use harness::{PtyHarness, WaitTimeout};
pub use keys::{encode_key, encode_paste, encode_sgr_mouse, Key, MouseButton};
pub use pty::{PtyController, PtySize};
pub use scenario::{Scenario, ScenarioOutcome, ScenarioRunner, Step};
pub use scenarios::{all_scenarios, run_named, smoke_scenario_ids, ScenarioId};
pub use screen::ScreenModel;
pub use timing::{TimingSample, TimingSummary};
