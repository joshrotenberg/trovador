//! trovador: local Claude Code and Codex sessions as one library, served through a CLI and
//! an MCP server. Both doors call the functions in [`sessions`]; neither computes its own
//! answer.

pub mod archive;
pub mod limits;
pub mod mcp;
pub mod paths;
pub mod sessions;
pub mod transcript;

pub use archive::Backend;
pub use limits::Limits;
pub use paths::Paths;
pub use sessions::Ctx;
