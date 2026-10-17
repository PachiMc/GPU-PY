//! GPU PY — monitor, overclock, ventiladores y LED/RGB para GPUs NVIDIA.
pub mod app;
pub mod autostart;
pub mod backend;
pub mod cli;
pub mod core;
pub mod error;
pub mod install;
pub mod nvapi;
pub mod reader;
#[cfg(windows)]
pub mod tray;
