//! Caveman's engine, ported from the Python toolkit in `tools/re/` (the reference implementation and test oracle;
//! see `docs/app-plan.md`). Each module names the Python module it ports.
//!
//! Module ownership during the port (parallel work; one owner per file):
//! - io: `oodle`, `cas`, `toc`, `bundle`, `ndb`, `game`
//! - ebx: `ebx`
//! - pixels: `texture`, `bc7`, `bc1`, `bcdec`, `thumbs`, `image`
//! - later: `brt`, `build`, `effects`, `shader`

pub mod anim;
pub mod bc1;
pub mod bc7;
pub mod bcdec;
pub mod boardmodel;
pub mod brt;
pub mod build;
pub mod bundle;
pub mod cas;
pub mod colour;
pub mod ebx;
pub mod effects;
pub mod fxmod;
pub mod game;
pub mod image;
pub mod import;
pub mod item_cache;
pub mod mesh;
pub mod ndb;
pub mod oodle;
pub mod rim;
pub mod settings;
pub mod shader;
pub mod shaderstate;
pub mod texture;
pub mod thumbs;
pub mod toc;
