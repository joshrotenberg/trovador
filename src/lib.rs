//! trovador: local Claude Code and Codex sessions as one library, served through a CLI and
//! an MCP server. Both doors call the functions in [`sessions`]; neither computes its own
//! answer.

pub mod archive;
mod codex;
pub mod doctor;
pub mod fields;
pub mod hook;
pub mod index;
pub mod init;
pub mod limits;
pub mod mcp;
pub mod output;
pub mod paths;
mod private;
mod provider;
pub mod redact;
pub mod revision;
pub mod schema;
pub mod sessions;
pub mod text;
pub mod timespec;
pub mod transcript;

/// The `tracing` target for progress a person at a terminal reads: what init is running,
/// how far indexing has got. The CLI prints it as plain lines on stderr when stderr is a
/// terminal or `TROVADOR_PROGRESS` is set; everything else in the library is a diagnostic
/// under `TROVADOR_LOG`.
pub const PROGRESS: &str = "trovador::progress";

pub use archive::Provider;
pub use limits::Limits;
pub use paths::Paths;
pub use sessions::Ctx;
