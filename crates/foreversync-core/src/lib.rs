//! Core of Forever Sync: finding the WoW Forever install and (later) the
//! snapshot / merge / apply engine for `WTF/`.

pub mod discovery;
pub mod forever;

pub use discovery::{Account, Character, DiscoveryError, ForeverInstall, Realm};
