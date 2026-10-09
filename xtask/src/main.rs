use std::{
    env,
    error::Error,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Host {
    Codex,
    Claude,
    Both,
}

impl Host {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    bin_name = "cargo xtask",
    about = "Build Tosi plugin packages"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Generate plugin/tosi and a host-specific archive in dist; does not install.
    Package(PackageOptions),
}

#[derive(Debug, Args)]
struct PackageOptions {
    /// Plugin host format; every binary still supports both worker backends.
    #[arg(long, value_enum, default_value = "both")]
    host: Host,
    /// Package --binary without building; you are responsible for freshness and native architecture.
    #[arg(long, requires = "binary")]
    skip_build: bool,
    /// Existing native executable; relative paths use the caller's working directory.
    #[arg(long, value_name = "PATH", requires = "skip_build")]
    binary: Option<PathBuf>,
}

struct Package {
    name: String,
    version: String,
    description: String,
}

fn cargo(root: &Path) -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root);
    command
}

fn metadata(root: &Path) -> Result<Package> {
    let output = cargo(root)
        .args(["metadata", "--no-deps", "--format-version=1", "--locked"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let manifest = root.join("Cargo.toml");
    let package = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata omitted packages")?
        .iter()
        .find(|p| {
            p["manifest_path"]
                .as_str()
                .is_some_and(|p| Path::new(p) == manifest)
        })
        .ok_or("root package missing from cargo metadata")?;
    Ok(Package {
        name: package["name"]
            .as_str()
            .ok_or("package name missing")?
            .into(),
        version: package["version"]
            .as_str()
            .ok_or("package version missing")?
            .into(),
        description: package["description"]
            .as_str()
            .unwrap_or("Local Claude and Codex tasks")
            .into(),
    })
}

fn build(root: &Path, package: &Package) -> Result<PathBuf> {
    let mut child = cargo(root)
        .args([
            "build",
            "--release",
            "--locked",
            "--package",
            &package.name,
            "--message-format=json-render-diagnostics",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut executable = None;
    for line in BufReader::new(child.stdout.take().ok_or("missing cargo stdout")?).lines() {
        let message: Value = serde_json::from_str(&line?)?;
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == package.name
            && let Some(path) = message["executable"].as_str()
        {
            executable = Some(PathBuf::from(path));
        }
    }
    if !child.wait()?.success() {
        return Err("release build failed".into());
    }
    executable.ok_or_else(|| "cargo did not report a tosi executable".into())
}

fn select_binary(root: &Path, package: &Package, options: &PackageOptions) -> Result<PathBuf> {
    if options.skip_build {
        let binary = options
            .binary
            .as_ref()
            .ok_or("--skip-build requires --binary PATH")?;
        Ok(std::path::absolute(binary)?)
    } else {
        build(root, package)
    }
}

fn write_json(path: &Path, value: Value) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or("missing parent directory")?)?;
    let mut file = fs::File::create(path)?;
    serde_json::to_writer_pretty(&mut file, &value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!("unsupported skill entry: {}", entry.path().display()).into());
        }
    }
    Ok(())
}

fn marketplace(package: &Package, source: &str) -> Value {
    json!({
        "name": "tosi-local",
        "description": "Personal local Claude and Codex delegation with Tosi.",
        "owner": { "name": "Josh Rotenberg" },
        "plugins": [{ "name": package.name, "description": package.description,
            "version": package.version, "source": source }]
    })
}

fn generate(
    root: &Path,
    destination: &Path,
    binary: &Path,
    package: &Package,
    host: Host,
) -> Result<()> {
    if !binary.is_file() {
        return Err(format!(
            "executable missing: {}; supply an existing --binary or run without --skip-build",
            binary.display()
        )
        .into());
    }
    fs::create_dir_all(destination.join("bin"))?;
    fs::copy(binary, destination.join("bin").join(&package.name))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            destination.join("bin").join(&package.name),
            fs::Permissions::from_mode(0o755),
        )?;
    }
    copy_tree(&root.join("resources/skills"), &destination.join("skills"))?;
    fs::copy(root.join("README.md"), destination.join("README.md"))?;
    if matches!(host, Host::Codex | Host::Both) {
        write_json(
            &destination.join("plugin.json"),
            json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": package.name, "version": package.version, "description": package.description,
                "author": { "name": "Josh Rotenberg" },
                "keywords": ["claude", "codex", "delegation", "local"],
                "extensions": { "com.openai": { "interface": {
                    "displayName": "Tosi", "shortDescription": "Claude and Codex delegation",
                    "longDescription": "Run, wait for, continue, and cancel bounded tasks using locally authenticated Claude and Codex command-line tools.",
                    "developerName": "Josh Rotenberg", "category": "Engineering", "capabilities": ["Read", "Write"],
                    "defaultPrompt": "Delegate an independent review of this project to Claude."
                }}}
            }),
        )?;
        write_json(
            &destination.join("mcp.json"),
            json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
                "mcpServers": { (package.name.clone()): {
                    "type": "stdio", "command": format!("./bin/{}", package.name), "args": []
                }}
            }),
        )?;
    }
    if matches!(host, Host::Claude | Host::Both) {
        write_json(
            &destination.join(".claude-plugin/plugin.json"),
            json!({
                "name": package.name, "version": package.version, "description": package.description,
                "author": { "name": "Josh Rotenberg" }, "keywords": ["claude", "codex", "delegation", "local"]
            }),
        )?;
        write_json(
            &destination.join(".mcp.json"),
            json!({
                "mcpServers": { (package.name.clone()): {
                    "type": "stdio", "command": format!("${{CLAUDE_PLUGIN_ROOT}}/bin/{}", package.name), "args": []
                }}
            }),
        )?;
    }
    // This catalog installs the extracted archive; the checkout catalog instead
    // points into plugin/. Both hosts understand this local catalog format.
    write_json(
        &destination.join(".claude-plugin/marketplace.json"),
        marketplace(package, "./"),
    )?;
    Ok(())
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn package(root: &Path, package: &Package, binary: &Path, host: Host) -> Result<PathBuf> {
    let output_dir = root.join("plugin");
    fs::create_dir_all(&output_dir)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stage = Stage(output_dir.join(format!(".package-{}-{nonce}", std::process::id())));
    fs::create_dir(&stage.0)?;
    let staged_plugin = stage.0.join(&package.name);
    generate(root, &staged_plugin, binary, package, host)?;
    let archive_name = format!(
        "{}-{}-{}-{}-{}.tar.gz",
        package.name,
        package.version,
        host.label(),
        env::consts::OS,
        env::consts::ARCH
    );
    let staged_archive = stage.0.join(&archive_name);
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&staged_archive)
        .arg("-C")
        .arg(&stage.0)
        .arg(&package.name)
        .status()?;
    if !status.success() {
        return Err("tar failed to create plugin archive".into());
    }
    let destination = output_dir.join(&package.name);
    let previous = stage.0.join("previous");
    if destination.exists() {
        fs::rename(&destination, &previous)?;
    }
    if let Err(error) = fs::rename(&staged_plugin, &destination) {
        if previous.exists() {
            fs::rename(previous, &destination)?;
        }
        return Err(error.into());
    }
    write_json(
        &root.join(".claude-plugin/marketplace.json"),
        marketplace(package, &format!("./plugin/{}", package.name)),
    )?;
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let archive = dist.join(archive_name);
    fs::rename(staged_archive, &archive)?;
    Ok(archive)
}

fn run() -> Result<()> {
    let Some(Action::Package(options)) = Cli::parse().command else {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    if !cfg!(unix) {
        return Err("packaging currently supports macOS and Linux".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("missing workspace root")?;
    let metadata = metadata(root)?;
    let binary = select_binary(root, &metadata, &options)?;
    let archive = package(root, &metadata, &binary, options.host)?;
    println!(
        "Generated {}",
        root.join("plugin").join(&metadata.name).display()
    );
    println!("Archive: {}", archive.display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_build_options() {
        let Some(Action::Package(defaults)) =
            Cli::try_parse_from(["xtask", "package"]).unwrap().command
        else {
            panic!("package subcommand missing");
        };
        assert_eq!(defaults.host, Host::Both);
        assert!(!defaults.skip_build);
        assert!(defaults.binary.is_none());
        let Some(Action::Package(explicit)) = Cli::try_parse_from([
            "xtask",
            "package",
            "--host",
            "claude",
            "--skip-build",
            "--binary",
            "build/tosi",
        ])
        .unwrap()
        .command
        else {
            panic!("package subcommand missing");
        };
        assert_eq!(explicit.host, Host::Claude);
        assert!(explicit.skip_build);
        assert_eq!(explicit.binary, Some(PathBuf::from("build/tosi")));
        for args in [
            vec!["xtask", "package", "--skip-build"],
            vec!["xtask", "package", "--binary", "build/tosi"],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap_err().kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
        }
        for args in [
            vec!["xtask", "package", "--host", "unknown"],
            vec!["xtask", "package", "--host"],
            vec!["xtask", "package", "--unknown"],
            vec!["xtask", "package", "unexpected"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        assert!(Cli::try_parse_from(["xtask"]).unwrap().command.is_none());
        for args in [
            vec!["xtask", "package", "--help"],
            vec!["xtask", "help", "package"],
        ] {
            let help = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(help.to_string().contains("--skip-build"));
            assert!(help.to_string().contains("--binary <PATH>"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn package_script_preserves_external_cwd_and_relative_binary_argument() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let caller = temp.path().join("caller workspace");
        let relative_binary = "chosen build/tosi";
        fs::create_dir_all(caller.join("chosen build")).unwrap();
        fs::write(caller.join(relative_binary), "caller-selected binary\n").unwrap();
        let mock_bin = temp.path().join("mock bin");
        fs::create_dir(&mock_bin).unwrap();
        let mock_cargo = mock_bin.join("cargo");
        fs::write(
            &mock_cargo,
            "#!/bin/sh\nset -eu\npwd -P\nprintf '%s\\n' \"$@\"\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = --binary ]; then\n    shift\n    cat -- \"$1\"\n    exit 0\n  fi\n  shift\ndone\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&mock_cargo, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![mock_bin];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let output = Command::new("sh")
            .arg(root.join("scripts/package.sh"))
            .args(["--skip-build", "--binary", relative_binary])
            .current_dir(&caller)
            .env("PATH", env::join_paths(paths).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let caller = caller.canonicalize().unwrap();
        let manifest = root.join("xtask/Cargo.toml").canonicalize().unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}\nrun\n--manifest-path\n{}\n--package\nxtask\n--\npackage\n--skip-build\n--binary\n{relative_binary}\ncaller-selected binary\n",
                caller.display(),
                manifest.display()
            )
        );
    }

    #[test]
    fn packages_each_host_without_stale_files_and_keeps_archive_self_contained() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("resources/skills/delegate/references")).unwrap();
        fs::write(
            root.join("resources/skills/delegate/SKILL.md"),
            "canonical skill\n",
        )
        .unwrap();
        fs::write(
            root.join("resources/skills/delegate/references/review.md"),
            "pattern\n",
        )
        .unwrap();
        fs::write(root.join("README.md"), "readme\n").unwrap();
        let stale_binary = root.join("target/release/tosi");
        fs::create_dir_all(stale_binary.parent().unwrap()).unwrap();
        fs::write(stale_binary, "stale executable").unwrap();
        let chosen_binary = root.join("target/native-triple/release/tosi");
        fs::create_dir_all(chosen_binary.parent().unwrap()).unwrap();
        fs::write(&chosen_binary, "chosen executable").unwrap();
        let metadata = Package {
            name: "tosi".into(),
            version: "9.8.7".into(),
            description: "fixture".into(),
        };
        let Some(Action::Package(options)) = Cli::try_parse_from([
            "xtask",
            "package",
            "--skip-build",
            "--binary",
            chosen_binary.to_str().unwrap(),
        ])
        .unwrap()
        .command
        else {
            panic!("package subcommand missing");
        };
        let binary = select_binary(root, &metadata, &options).unwrap();
        for host in [Host::Both, Host::Codex, Host::Claude] {
            let archive = package(root, &metadata, &binary, host).unwrap();
            let plugin = root.join("plugin/tosi");
            assert_eq!(
                fs::read(plugin.join("bin/tosi")).unwrap(),
                b"chosen executable"
            );
            assert_eq!(
                fs::read_to_string(plugin.join("skills/delegate/SKILL.md")).unwrap(),
                "canonical skill\n"
            );
            assert_eq!(
                fs::read_to_string(plugin.join("skills/delegate/references/review.md")).unwrap(),
                "pattern\n"
            );
            assert_eq!(plugin.join("plugin.json").exists(), host != Host::Claude);
            assert_eq!(plugin.join("mcp.json").exists(), host != Host::Claude);
            assert_eq!(
                plugin.join(".claude-plugin/plugin.json").exists(),
                host != Host::Codex
            );
            assert_eq!(plugin.join(".mcp.json").exists(), host != Host::Codex);
            for manifest in ["plugin.json", ".claude-plugin/plugin.json"] {
                if plugin.join(manifest).exists() {
                    let value: Value =
                        serde_json::from_slice(&fs::read(plugin.join(manifest)).unwrap()).unwrap();
                    assert_eq!(value["name"], "tosi");
                    assert_eq!(value["version"], "9.8.7");
                    if manifest == "plugin.json" {
                        assert!(
                            value["extensions"]["com.openai"]["interface"]["shortDescription"]
                                .as_str()
                                .unwrap()
                                .chars()
                                .count()
                                <= 30
                        );
                        assert_eq!(
                            value["extensions"]["com.openai"]["interface"]["displayName"],
                            "Tosi"
                        );
                    }
                }
            }
            for (catalog, source) in [
                (
                    root.join(".claude-plugin/marketplace.json"),
                    "./plugin/tosi",
                ),
                (plugin.join(".claude-plugin/marketplace.json"), "./"),
            ] {
                let value: Value = serde_json::from_slice(&fs::read(catalog).unwrap()).unwrap();
                assert_eq!(value["name"], "tosi-local");
                assert_eq!(value["plugins"][0]["name"], "tosi");
                assert_eq!(value["plugins"][0]["source"], source);
            }
            for (config, command) in [
                ("mcp.json", "./bin/tosi"),
                (".mcp.json", "${CLAUDE_PLUGIN_ROOT}/bin/tosi"),
            ] {
                if plugin.join(config).exists() {
                    let value: Value =
                        serde_json::from_slice(&fs::read(plugin.join(config)).unwrap()).unwrap();
                    assert_eq!(value["mcpServers"]["tosi"]["command"], command);
                }
            }
            let entries = Command::new("tar")
                .arg("-tzf")
                .arg(&archive)
                .output()
                .unwrap();
            assert!(entries.status.success());
            let entries = String::from_utf8(entries.stdout).unwrap();
            assert!(entries.lines().all(|entry| entry.starts_with("tosi/")));
            assert!(entries.contains("tosi/bin/tosi\n"));
            assert!(entries.contains("tosi/.claude-plugin/marketplace.json\n"));
            assert!(!entries.contains("run-tosi.sh"));
            let archived_binary = Command::new("tar")
                .arg("-xOzf")
                .arg(&archive)
                .arg("tosi/bin/tosi")
                .output()
                .unwrap();
            assert!(archived_binary.status.success());
            assert_eq!(archived_binary.stdout, b"chosen executable");
            assert_eq!(
                archive.file_name().unwrap().to_str().unwrap(),
                format!(
                    "tosi-9.8.7-{}-{}-{}.tar.gz",
                    host.label(),
                    env::consts::OS,
                    env::consts::ARCH
                )
            );
        }
    }

    #[test]
    fn missing_binary_preserves_existing_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("plugin/tosi")).unwrap();
        fs::write(root.join("plugin/tosi/keep"), "existing package").unwrap();
        let metadata = Package {
            name: "tosi".into(),
            version: "1.0.0".into(),
            description: "fixture".into(),
        };
        let options = PackageOptions {
            host: Host::Both,
            skip_build: true,
            binary: Some(PathBuf::from("missing-tosi-executable")),
        };
        let binary = select_binary(root, &metadata, &options).unwrap();
        assert_eq!(
            binary,
            env::current_dir().unwrap().join("missing-tosi-executable")
        );
        assert!(package(root, &metadata, &binary, options.host).is_err());
        assert_eq!(
            fs::read_to_string(root.join("plugin/tosi/keep")).unwrap(),
            "existing package"
        );
        assert_eq!(fs::read_dir(root.join("plugin")).unwrap().count(), 1);
    }
}
