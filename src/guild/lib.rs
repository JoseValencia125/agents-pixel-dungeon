//! Guild: the work layer of Agents Pixel Dungeon. Quests are split into
//! tasks, each task is attempted by a guild member (a role bound to a
//! harness and a model) inside its own git worktree, and only candidates
//! the user or a verifier accepts are integrated. The service owns every
//! state transition; harnesses only propose. The widget and the CLI are
//! clients of its Unix socket.

pub mod api;
pub mod config;
pub mod domain;
pub mod harness;
pub mod paths;
pub mod scheduler;
pub mod service;
pub mod store;
pub mod workspaces;

pub use domain::*;
