//! # tpt-fab
//!
//! Wafer-fabrication SECS/GEM control stack: the equipment-communication, recipe, lot, and
//! process-job layer that sits between a finished layout and an actual wafer.
//!
//! ## Modules
//!
//! * [`hsms`] — HSMS message transport (SEMI E37): framing, select/linktest/separate control
//!   transactions, T3/T5/T6/T7/T8 timers, and the `NOT_CONNECTED → CONNECTED → SELECTED` state
//!   machine over TCP.
//! * [`secs`] — SECS-II message encode/decode (SEMI E5): all data item formats plus the 10-byte
//!   message header shared with HSMS frames.
//! * [`gem`] — GEM (SEMI E30) state models (communication + control), SVID/ECID/CEID/ALID
//!   registries, alarms, terminal services, and host-message handling.
//! * [`gem300`] — the GEM300 suite: E39 object services, E40 process jobs, E90 substrate
//!   tracking, E116 data collection.
//! * [`recipe`] — recipe data model with append-only versioning, body checksums, and drift
//!   detection against per-parameter tolerances.
//! * [`lot`] — lot tracking with CEID-annotated state history.
//! * [`adapter`] — the opt-in seam for vendor-proprietary SECS/GEM extensions. Core behavior is
//!   pure SEMI-standard unless an adapter is explicitly installed at runtime.
//! * [`sim`] — the equipment-simulator harness: a TCP-speaking GEM equipment plus a host-side
//!   validation client, so the whole stack runs end-to-end without real tool access.
//!
//! ## Technology neutrality
//!
//! The core never matches on lithography technology. Everything technology-specific lives
//! behind [`tpt_fab_litho::PatterningBackend`], which the simulator invokes per substrate;
//! swapping the installed backend requires no changes to recipe, lot, or job logic.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod error;
pub mod gem;
pub mod gem300;
pub mod hsms;
pub mod lot;
pub mod recipe;
pub mod secs;
pub mod sim;

pub use error::FabError;
pub use tpt_fab_litho::PatterningBackend;
