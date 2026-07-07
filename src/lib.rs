//! `lease_sim` — a simulation kit of distributed lease algorithm messaging.
//!
//! A discrete-event, multi-node lease message simulator. Build a [`Scenario`],
//! run it through the [`Engine`], and consume the resulting timestamped
//! [`Event`] stream or interpolated [`Frame`] geometry. See the design in
//! `docs/design/simulator.md` and the algorithms in `docs/design/algorithm.md`.

pub mod clock;
pub mod dist;
pub mod engine;
pub mod event;
pub mod frame;
pub mod rng;
pub mod scenario;

pub use clock::{Clock, Time};
pub use dist::Dist;
pub use engine::Engine;
pub use event::{Command, Event, EventKind, LeaseId, LeaseStatus, MsgFate, MsgKind, NodeId};
pub use frame::{Frame, LeaseBar, MsgShape, NodeShape, NodeViz, Point};
pub use rng::Rng;
pub use scenario::{LeaseParams, LinkConfig, NodeConfig, Scenario};
