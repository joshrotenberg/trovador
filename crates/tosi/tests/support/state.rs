use std::path::Path;

pub fn server_command(binary: impl AsRef<Path>, root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary.as_ref());
    // Binary tests must not resolve shared provenance in the developer's home.
    let state_root = root.join(".bottega");
    assert!(state_root.starts_with(root));
    command
        .env("BOTTEGA_TEST_ROOT", root)
        .env("BOTTEGA_HOME", state_root)
        .env_remove("TROVADOR_HOME");
    command
}
