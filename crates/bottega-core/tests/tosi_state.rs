use bottega_core::state::{prepare, resolve};
#[test]
fn tosi_state_is_private_and_resolution_has_no_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let state = resolve(
        Some(temp.path().join("root").into_os_string()),
        None,
        temp.path(),
    );
    assert!(state.tosi.starts_with(temp.path()));
    assert!(!state.root.exists());
    let state = prepare(state).unwrap();
    assert_eq!(state.tosi, temp.path().join("root/tosi"));
    assert!(state.tosi.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state.tosi).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&state.tosi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = prepare(state).unwrap();
        assert_eq!(
            std::fs::metadata(state.tosi).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
