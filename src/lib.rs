//! trovador: local Claude Code and Codex sessions as one library, served through a CLI and
//! an MCP server. Both doors call the functions in [`sessions`]; neither computes its own
//! answer.

pub mod archive;
mod backend;
mod codex;
pub mod doctor;
pub mod hook;
pub mod index;
pub mod init;
pub mod limits;
pub mod mcp;
pub mod output;
pub mod paths;
mod private;
pub mod redact;
pub mod revision;
pub mod sessions;
pub mod text;
pub mod timespec;
pub mod transcript;

pub use archive::Backend;
pub use limits::Limits;
pub use paths::Paths;
pub use sessions::Ctx;
