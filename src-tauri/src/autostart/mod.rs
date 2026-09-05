//! Starting the app at login: opt-in, off by default, current-user only, on every platform.
//!
//! Each backend enforces "current user only, no admin" through whatever mechanism the OS
//! itself scopes that way — Linux writes into `$XDG_CONFIG_HOME`, which is already
//! per-user; Windows writes into `HKEY_CURRENT_USER`, which is likewise never shared and
//! never needs elevation. Neither backend shares code with the other beyond the shape of
//! `sync`, because the two mechanisms have nothing else in common: one is a text file, the
//! other a registry value.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
