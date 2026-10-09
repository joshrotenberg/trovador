//! Compatibility adapters keep callers using their explicit archive paths.
use crate::{Backend, Paths};
pub use bottega_core::provenance::{ProvenanceError, Status};
pub fn path(paths: &Paths) -> std::path::PathBuf {
    bottega_core::provenance::path(&paths.shared)
}
pub fn record_spawned(paths: &Paths, backend: Backend, id: &str) -> Result<(), ProvenanceError> {
    bottega_core::provenance::record_spawned(&paths.shared, backend, id)
}
pub fn record_all(
    paths: &Paths,
    incoming: &[(Backend, String)],
    dry_run: bool,
) -> Result<(usize, usize), ProvenanceError> {
    bottega_core::provenance::record_all(&paths.shared, incoming, dry_run)
}
pub fn is_spawned(paths: &Paths, backend: Backend, id: &str) -> bool {
    bottega_core::provenance::is_spawned(&paths.shared, backend, id)
}
pub fn spawned_keys(paths: &Paths) -> Vec<String> {
    bottega_core::provenance::spawned_keys(&paths.shared)
}
pub fn status(paths: &Paths) -> Status {
    bottega_core::provenance::status(&paths.shared)
}
