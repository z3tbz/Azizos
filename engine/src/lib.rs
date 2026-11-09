//! Azizos engine — declarative Windows 10/11 tweak applicator.
//!
//! This crate is split so the UI (Tauri) and the CLI share one core:
//!   schema  — the tweak data model (platform-independent, unit-tested here)
//!   catalog — load/validate tweak definitions, filter by system, resolve order
//!   (later) detect — read-only system + requirements probing (Windows only)
//!   (later) exec   — action executors over the Windows API (Windows only)
//!
//! Nothing in `schema`/`catalog` touches the system, so it builds and tests on
//! any platform; the Windows-only modules are gated behind `#[cfg(windows)]`.

pub mod schema;
pub mod catalog;
pub mod exec;
pub mod detect;

pub use schema::{Action, Check, Compat, Device, Disk, Gpu, Hive, Risk, Tweak};
pub use catalog::{Catalog, Plan, PlanError, SystemProfile};
pub use exec::{Backend, Phase, Report, Status};
pub use detect::{check_requirements, Detector, Met, Remedy, Requirement, RequirementStatus};
