//! Addon sync through WowUp (DESIGN.md §6).
//!
//! Forever Sync never touches addon files. It reads WowUp's records to learn
//! which addons are installed, and turns that list into a WowUp import string.
//! Folders WowUp doesn't manage are reported, never synced.

pub mod import;
pub mod wowup;

pub use import::{ImportAddon, ImportPayload, import_string};
pub use wowup::{Inventory, ManagedAddon, WowUpError, default_wowup_dirs, load_inventory};
