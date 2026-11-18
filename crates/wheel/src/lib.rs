//! The wheel, minus pixels (docs/02 §3, docs/04 §3–5).
//!
//! Platform layers turn OS input into [`fsm::Event`]s and carry out [`fsm::Effect`]s;
//! renderers draw what [`geom`] lays out. Nothing here touches the OS, so every rule is
//! unit- and property-tested on any host.

pub mod fsm;
pub mod geom;
pub mod rank;

pub use fsm::{DropEffect, Effect, Event, Key, Machine, State, WedgeShape, Wheel};
pub use geom::{SubRing, Zone};
pub use rank::{Frecency, assign_slots};
