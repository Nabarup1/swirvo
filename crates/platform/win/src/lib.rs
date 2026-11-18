//! Windows platform layer. `model` is portable (tested everywhere); `agent` is the Win32
//! overlay for spike S1 (docs/08): trigger keys → invisible OLE probe → wheel → job.

pub mod model;
pub mod pathlist;

#[cfg(windows)]
mod agent;
#[cfg(windows)]
mod apps;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod shells;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod util;
#[cfg(windows)]
mod window;
#[cfg(windows)]
pub use agent::run;
