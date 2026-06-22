use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PERDIR_DIR: &str = ".perdir";
const WORLD_FILE: &str = "world.toml";
const LOG_FILE: &str = "audit.log";

#[derive(Parser, Debug)]
#[command(name = "perdir")]
#[command(about = "Per-directory Linux environments", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a per-directory environment in the current directory.
    Init {
        /// Human-readable environment name. Defaults to the directory name.
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Show the active perdir environment manifest.
    Status,
    /// Run a command inside this directory's declared environment.
    Run {
        /// Command and arguments to execute.
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Print shell commands to enter the environment manually.
    Enter,
    /// Explain the current environment manifest.
    Explain,
    /// Show the audit log for this environment.
    Log,
    /// Open the environment manifest in $EDITOR.
    Edit,
}

#[derive(Debug, Serialize, Deserialize)]
struct World {
    name: String,
    runtime: Runtime,
    permissions: Permissions,
    ai: Ai,
}

#[derive(Debug, Serialize, Deserialize)]
struct Runtime {
    python: Option<String>,
    packages: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Permissions {
    network: PermissionMode,
    home: PermissionMode,
    gpu: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PermissionMode {
    Allow,
    Ask,
    Deny,
    ReadOnly,
}

#[derive(Debug, Serialize, Deserialize)]
struct Ai {
    context: Vec<String>,
    memory_file: String,
    model: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { name } => init(name),
        Commands::Status => status(),
        Commands::Run { command } => run(command),
        Commands::Enter => enter(),
        Commands::Explain => explain(),
        Commands::Log => log(),
        Commands::Edit => edit(),
    }
}

fn init(name: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_name = name.unwrap_or_else(|| {
        cwd.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "perdir-project".to_string())
    });

    let perdir_path = cwd.join(PERDIR_DIR);
    fs::create_dir_all(&perdir_path)?;

    let world_path = perdir_path.join(WORLD_FILE);
    if world_path.exists() {
        return Err(anyhow!("{} already exists", world_path.display()));
    }

    let world = World {
        name: project_name,
        runtime: Runtime {
            python: Some("3.12".to_string()),
            packages: vec!["python".to_string()],
            env: Default::default(),
        },
        permissions: Permissions {
            network: PermissionMode::Ask,
            home: PermissionMode::ReadOnly,
            gpu: false,
        },
        ai: Ai {
            context: vec!["README.md".to_string(), "src/".to_string()],
            memory_file: ".perdir/memory.md".to_string(),
            model: "local-or-cloud".to_string(),
        },
    };

    fs::write(&world_path, toml::to_string_pretty(&world)?)?;
    fs::write(perdir_path.join("memory.md"), "# Perdir Memory\n\n")?;
    append_log(&cwd, "init")?;

    println!("Initialized perdir environment at {}", world_path.display());
    Ok(())
}

fn status() -> Result<()> {
    let (root, world) = load_world()?;
    println!("Directory: {}", root.display());
    println!("{}", toml::to_string_pretty(&world)?);
    Ok(())
}

fn run(command: Vec<String>) -> Result<()> {
    if command.is_empty() {
        return Err(anyhow!("missing command"));
    }

    let (root, world) = load_world()?;
    append_log(&root, &format!("run {:?}", command))?;

    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for (key, value) in world.runtime.env.iter() {
        cmd.env(key, value);
    }

    // MVP behavior: construct a predictable PERDIR_ROOT and PERDIR_NAME.
    // Future behavior: use bubblewrap/nix namespaces before exec.
    cmd.env("PERDIR_ROOT", root.to_string_lossy().to_string());
    cmd.env("PERDIR_NAME", world.name);

    let status = cmd.status().context("failed to spawn command")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn enter() -> Result<()> {
    let (root, world) = load_world()?;
    println!("export PERDIR_ROOT='{}'", root.display());
    println!("export PERDIR_NAME='{}'", shell_escape(&world.name));
    for (key, value) in world.runtime.env {
        println!("export {}='{}'", key, shell_escape(&value));
    }
    println!("# Run this to enter:");
    println!("# eval \"$(perdir enter)\"");
    Ok(())
}

fn log() -> Result<()> {
    let (root, _world) = load_world()?;
    let log_path = root.join(PERDIR_DIR).join(LOG_FILE);
    let contents = fs::read_to_string(&log_path)
        .with_context(|| format!("could not read {}", log_path.display()))?;
    print!("{}", contents);
    Ok(())
}

fn edit() -> Result<()> {
    let (root, _world) = load_world()?;
    let manifest_path = root.join(PERDIR_DIR).join(WORLD_FILE);
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&manifest_path)
        .status()
        .with_context(|| format!("failed to launch editor: {}", editor))?;
    if !status.success() {
        return Err(anyhow!("editor exited with non-zero status"));
    }
    Ok(())
}

fn explain() -> Result<()> {
    let (_root, world) = load_world()?;
    println!(
        "This directory declares an environment named '{}'.",
        world.name
    );
    println!("Runtime packages: {}", world.runtime.packages.join(", "));
    println!("Network permission: {:?}", world.permissions.network);
    println!("Home permission: {:?}", world.permissions.home);
    println!("GPU access: {}", world.permissions.gpu);
    println!("AI context paths: {}", world.ai.context.join(", "));
    println!("AI memory file: {}", world.ai.memory_file);
    Ok(())
}

fn load_world() -> Result<(PathBuf, World)> {
    let root = find_world_root(std::env::current_dir()?)?;
    let manifest_path = root.join(PERDIR_DIR).join(WORLD_FILE);
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("could not read {}", manifest_path.display()))?;
    let world: World = toml::from_str(&raw)
        .with_context(|| format!("could not parse {}", manifest_path.display()))?;
    Ok((root, world))
}

fn find_world_root(start: PathBuf) -> Result<PathBuf> {
    for candidate in start.ancestors() {
        if candidate.join(PERDIR_DIR).join(WORLD_FILE).exists() {
            return Ok(candidate.to_path_buf());
        }
    }
    Err(anyhow!(
        "not inside a perdir environment; run `perdir init` first"
    ))
}

fn append_log(root: &Path, action: &str) -> Result<()> {
    let log_path = root.join(PERDIR_DIR).join(LOG_FILE);
    let line = format!("{} {}\n", Utc::now().to_rfc3339(), action);
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?
        .write_all(line.as_bytes())?;
    Ok(())
}

fn shell_escape(input: &str) -> String {
    input.replace('\\', "\\\\").replace('\'', "'\\''")
}
