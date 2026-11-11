//! Azizos engine — declarative Windows 10/11 tweak applicator.
//!
//! This crate is split so the UI (Tauri) and the CLI share one core:
//!   schema  — the tweak data model (platform-independent, unit-tested here)
//!   catalog — load/validate tweak definitions, filter by system, resolve order
//!   exec    — resilient orchestration over a `Backend` trait (pure, unit-tested)
//!   detect  — pre-flight requirement evaluation over a `Detector` trait (pure)
//!   exec_windows / detect_windows — the real Windows API implementations
//!
//! Nothing in `schema`/`catalog` touches the system, so it builds and tests on
//! any platform; the Windows-only modules are gated behind `#[cfg(windows)]`.

pub mod schema;
pub mod catalog;
pub mod exec;
pub mod detect;
pub mod diagnostic;

#[cfg(all(windows, feature = "native"))]
pub mod exec_windows;
#[cfg(all(windows, feature = "native"))]
pub mod detect_windows;
#[cfg(all(windows, feature = "native"))]
pub mod win32;
#[cfg(all(windows, feature = "native"))]
pub mod appx;
#[cfg(all(windows, feature = "native"))]
pub mod com;
#[cfg(all(windows, feature = "native"))]
pub mod cache;

#[cfg(all(windows, feature = "native"))]
pub use exec_windows::WindowsBackend;
#[cfg(all(windows, feature = "native"))]
pub use detect_windows::WindowsDetector;

pub use schema::{Action, Check, Compat, Device, Disk, Gpu, Hive, Risk, Tweak};
pub use catalog::{Catalog, Plan, PlanError, SystemProfile};
pub use exec::{Applied, Backend, Counts, Phase, Report, Status};
pub use detect::{check_requirements, Detector, Met, Remedy, Requirement, RequirementStatus};
