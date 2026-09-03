//! Git layer. Reads and index writes through git2; network through the git
//! CLI (Phase 4). The UI only ever sees an immutable [`RepoSnapshot`].

pub mod graph;
pub mod ops;
pub mod repo;
