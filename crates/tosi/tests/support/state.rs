use std::path::Path;

pub fn server_command(binary: impl AsRef<Path>, root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary.as_ref());
    // Binary tests must not resolve shared provenance in the developer's home.
    command.env("TROVADOR_HOME", root.join(".trovador"));
    command
}
