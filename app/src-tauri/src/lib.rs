//! The Caveman app's back end, as a library so it can be tested without a window: the configuration, the library scan
//! and settings files, the fitted art for the 3D preview, and the build and launch.

pub mod art;
pub mod bake;
pub mod config;
pub mod library;
pub mod live;
pub mod loadout;
pub mod ring;
pub mod sqlite;
pub mod widgets;
