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
    /// Validate the environment manifest for issues.
    Validate,
    /// Print shell integration script for auto-activation on cd.
    ShellInit,
    /// Clean the venv and package marker, forcing a fresh rebuild on next run.
    Clean,
    /// Create the venv and install packages from the manifest.
    Install,
    /// Ask AI to propose manifest changes based on context files and a prompt.
    Ai {
        /// The request or question for the AI.
        #[arg(required = true, trailing_var_arg = true)]
        prompt: Vec<String>,
    },
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
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    pip_packages: Vec<String>,
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
        Commands::Validate => validate(),
        Commands::ShellInit => shell_init(),
        Commands::Clean => clean(),
        Commands::Install => install(),
        Commands::Ai { prompt } => ai(prompt),
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
            pip_packages: vec![],
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

    if !check_permissions(&world) {
        return Ok(());
    }

    let venv_bin = activate_venv(&root, &world);

    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for (key, value) in world.runtime.env.iter() {
        cmd.env(key, value);
    }

    cmd.env("PERDIR_ROOT", root.to_string_lossy().to_string());
    cmd.env("PERDIR_NAME", world.name.clone());

    if let Some(bin) = &venv_bin {
        let current_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{}", bin.display(), current_path));
        cmd.env("VIRTUAL_ENV", bin.parent().unwrap().display().to_string());
    }

    apply_permission_env(&root, &world, &mut cmd);

    let status = cmd.status().context("failed to spawn command")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn enter() -> Result<()> {
    let (root, world) = load_world()?;
    println!("export PERDIR_ROOT='{}'", root.display());
    println!("export PERDIR_NAME='{}'", shell_escape(&world.name));
    for (key, value) in &world.runtime.env {
        println!("export {}='{}'", key, shell_escape(value));
    }
    let venv_bin = activate_venv(&root, &world);
    if let Some(bin) = &venv_bin {
        let current_path = std::env::var("PATH").unwrap_or_default();
        println!(
            "export PATH='{}:{}'",
            bin.display(),
            shell_escape(&current_path)
        );
        println!("export VIRTUAL_ENV='{}'", bin.parent().unwrap().display());
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

fn validate() -> Result<()> {
    let (root, world) = load_world()?;
    let issues = validate_world(&root, &world);
    if issues.is_empty() {
        println!("Manifest is valid. No issues found.");
    } else {
        for issue in &issues {
            println!("{}", issue);
        }
        std::process::exit(1);
    }
    Ok(())
}

fn validate_world(root: &Path, world: &World) -> Vec<String> {
    let mut issues = Vec::new();

    if world.name.trim().is_empty() {
        issues.push("ERROR: name is empty".to_string());
    }

    if world.runtime.packages.is_empty() && world.runtime.pip_packages.is_empty() {
        issues.push("WARN: no runtime packages declared".to_string());
    }

    if let Some(ref py) = world.runtime.python {
        if !py.chars().all(|c| c.is_ascii_digit() || c == '.') {
            issues.push(format!(
                "WARN: python version '{}' contains unexpected characters",
                py
            ));
        }
    }

    for ctx_path in &world.ai.context {
        let full = root.join(ctx_path);
        if !full.exists() {
            issues.push(format!(
                "WARN: ai context path '{}' does not exist",
                ctx_path
            ));
        }
    }

    if world.ai.memory_file.trim().is_empty() {
        issues.push("ERROR: ai.memory_file is empty".to_string());
    } else {
        let mem_full = root.join(&world.ai.memory_file);
        if !mem_full.exists() {
            issues.push(format!(
                "WARN: ai memory file '{}' does not exist",
                world.ai.memory_file
            ));
        }
    }

    if world.ai.model.trim().is_empty() {
        issues.push("WARN: ai.model is empty".to_string());
    }

    issues
}

fn explain() -> Result<()> {
    let (_root, world) = load_world()?;
    println!(
        "This directory declares an environment named '{}'.",
        world.name
    );
    println!("Runtime packages: {}", world.runtime.packages.join(", "));
    println!("Pip packages: {}", world.runtime.pip_packages.join(", "));
    println!("Network permission: {:?}", world.permissions.network);
    println!("Home permission: {:?}", world.permissions.home);
    println!("GPU access: {}", world.permissions.gpu);
    println!("AI context paths: {}", world.ai.context.join(", "));
    println!("AI memory file: {}", world.ai.memory_file);
    Ok(())
}

fn shell_init() -> Result<()> {
    let script = r#"# perdir shell integration — add to ~/.zshrc or ~/.bashrc:
#   eval "$(perdir shell-init)"

__perdir_hook() {
    if [ -f ".perdir/world.toml" ]; then
        if [ -z "$PERDIR_ROOT" ] || [ "$PERDIR_ROOT" != "$(pwd)" ]; then
            eval "$(perdir enter 2>/dev/null)"
        fi
    elif [ -n "$PERDIR_ROOT" ]; then
        unset PERDIR_ROOT PERDIR_NAME VIRTUAL_ENV
    fi
}

# zsh uses chpwd_functions, bash overrides PROMPT_COMMAND
if [ -n "$ZSH_VERSION" ]; then
    chpwd_functions=(__perdir_hook $chpwd_functions)
    __perdir_hook
elif [ -n "$BASH_VERSION" ]; then
    __perdir_prompt_cmd() { __perdir_hook; }
    PROMPT_COMMAND="__perdir_prompt_cmd;$PROMPT_COMMAND"
    __perdir_hook
fi
"#;
    print!("{}", script);
    Ok(())
}

fn clean() -> Result<()> {
    let (root, _world) = load_world()?;
    clean_venv(&root)
}

fn clean_venv(root: &Path) -> Result<()> {
    let venv_dir = root.join(PERDIR_DIR).join("venv");

    if !venv_dir.exists() {
        println!("No venv found at {}. Nothing to clean.", venv_dir.display());
        return Ok(());
    }

    fs::remove_dir_all(&venv_dir)
        .with_context(|| format!("failed to remove {}", venv_dir.display()))?;
    append_log(root, "clean")?;
    println!(
        "Removed venv at {}. Run `perdir install` to recreate it.",
        venv_dir.display()
    );
    Ok(())
}

fn activate_venv(root: &Path, world: &World) -> Option<PathBuf> {
    world.runtime.python.as_ref()?;

    let venv_bin = root.join(PERDIR_DIR).join("venv").join("bin");
    if venv_bin.exists() {
        Some(venv_bin)
    } else {
        eprintln!("[perdir] No venv found. Run `perdir install` to create one.");
        None
    }
}

fn install() -> Result<()> {
    let (root, world) = load_world()?;
    let venv_bin = create_venv(&root, &world)?;
    if let Some(bin) = &venv_bin {
        install_packages(&root, &world, bin)?;
    }
    append_log(&root, "install")?;
    println!("Environment ready. Use `perdir run <command>` to execute commands.");
    Ok(())
}

fn create_venv(root: &Path, world: &World) -> Result<Option<PathBuf>> {
    let python = match &world.runtime.python {
        Some(p) => p,
        None => return Ok(None),
    };

    let venv_dir = root.join(PERDIR_DIR).join("venv");
    let venv_bin = venv_dir.join("bin");

    if !venv_bin.exists() {
        let py_bin = format!("python{}", python);
        let python_cmd = which::which(&py_bin)
            .or_else(|_| which::which("python3"))
            .map_err(|_| anyhow!("no python interpreter found for venv creation"))?;

        println!("Creating venv at {} ...", venv_dir.display());
        let status = Command::new(&python_cmd)
            .arg("-m")
            .arg("venv")
            .arg(&venv_dir)
            .status()
            .context("failed to spawn python venv creation")?;
        if !status.success() {
            return Err(anyhow!("venv creation failed"));
        }
    }

    Ok(Some(venv_bin))
}

fn install_packages(root: &Path, world: &World, venv_bin: &Path) -> Result<()> {
    let marker = venv_bin.parent().unwrap().join(".perdir_packages");
    let packages_json = serde_json::to_string(&world.runtime.pip_packages)?;

    let needs_install = match fs::read_to_string(&marker) {
        Ok(prev) => prev != packages_json,
        Err(_) => true,
    };

    if needs_install && !world.runtime.pip_packages.is_empty() {
        let pip = venv_bin.join("pip");
        println!(
            "Installing packages: {} ...",
            world.runtime.pip_packages.join(", ")
        );
        let status = Command::new(&pip)
            .arg("install")
            .args(&world.runtime.pip_packages)
            .status()
            .context("failed to spawn pip install")?;
        if !status.success() {
            return Err(anyhow!("pip install failed"));
        }
        fs::write(&marker, &packages_json)?;
    }

    write_lock_file(root, venv_bin)?;
    Ok(())
}

fn write_lock_file(root: &Path, venv_bin: &Path) -> Result<()> {
    let pip = venv_bin.join("pip");
    let output = Command::new(&pip)
        .arg("list")
        .arg("--format=freeze")
        .output()
        .context("failed to run pip list")?;
    let lock_path = root.join(PERDIR_DIR).join("perdir.lock");
    fs::write(&lock_path, &output.stdout)?;
    Ok(())
}

fn ai(prompt: Vec<String>) -> Result<()> {
    let (root, world) = load_world()?;
    let user_prompt = prompt.join(" ");
    append_log(&root, &format!("ai {:?}", user_prompt))?;

    let manifest = toml::to_string_pretty(&world)?;

    let mut context = String::new();
    for ctx_path in &world.ai.context {
        let full = root.join(ctx_path);
        if full.is_dir() {
            for entry in fs::read_dir(&full)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        context.push_str(&format!("--- {} ---\n{}\n\n", ctx_path, content));
                    }
                }
            }
        } else if let Ok(content) = fs::read_to_string(&full) {
            context.push_str(&format!("--- {} ---\n{}\n\n", ctx_path, content));
        }
    }

    let memory = fs::read_to_string(root.join(&world.ai.memory_file))
        .unwrap_or_else(|_| "# No memory file\n".to_string());

    let system = format!(
        "You are perdir's AI assistant. You help users manage their per-directory environment manifest.\n\
         The manifest is a TOML file called world.toml with this structure:\n\
         - name: string\n\
         - [runtime]: python (optional version string), packages (system-level), pip_packages (PyPI packages), env (key-value map)\n\
         - [permissions]: network (allow/ask/deny), home (allow/ask/deny/read-only), gpu (bool)\n\
         - [ai]: context (file paths), memory_file, model\n\n\
         When the user asks you to modify the manifest, output the COMPLETE updated manifest in a ```toml code block.\n\
         Do not include any explanation outside the code block if you are proposing changes.\n\
         If you are only answering a question, respond normally.\n\n\
         Current manifest:\n```toml\n{}\n```\n\n\
         Memory:\n{}\n\n\
         Context files:\n{}",
        manifest, memory, context
    );

    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow!("OPENAI_API_KEY not set. Set it to use perdir ai."))?;
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = if world.ai.model.trim().is_empty() || world.ai.model == "local-or-cloud" {
        "gpt-4o".to_string()
    } else {
        world.ai.model.clone()
    };

    println!("[perdir] Asking {} ...", model);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{}/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_prompt},
            ],
            "temperature": 0.3,
        }))
        .send()
        .context("failed to send request to AI API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("AI API error ({}): {}", status, body));
    }

    let body: serde_json::Value = resp.json().context("failed to parse AI API response")?;
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("unexpected AI API response format"))?;

    if let Some(toml_block) = extract_toml_block(content) {
        println!("Proposed manifest:\n");
        println!("```toml");
        println!("{}", toml_block);
        println!("```");

        eprint!("\nApply this manifest? [y/N] ");
        std::io::stderr().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if input.trim().eq_ignore_ascii_case("y") {
            let world_path = root.join(PERDIR_DIR).join(WORLD_FILE);
            fs::write(&world_path, &toml_block)?;
            append_log(&root, "ai-apply")?;
            println!("Manifest updated. Run `perdir install` to apply changes.");
        } else {
            println!("Not applied.");
        }
    } else {
        println!("{}", content);
    }

    Ok(())
}

fn extract_toml_block(content: &str) -> Option<String> {
    let start = content.find("```toml")?;
    let after_start = &content[start + 7..];
    let end = after_start.find("```")?;
    Some(after_start[..end].trim().to_string())
}

fn check_permissions(world: &World) -> bool {
    if matches!(world.permissions.network, PermissionMode::Ask) {
        eprint!("[perdir] Network access is set to 'ask'. Allow network for this command? [y/N] ");
        std::io::stderr().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("[perdir] Aborted by user.");
            return false;
        }
    }

    if let PermissionMode::Deny = world.permissions.network {
        eprintln!("[perdir] Network access is denied by manifest policy — all network sockets will be blocked");
    }

    match world.permissions.home {
        PermissionMode::Deny => {
            eprintln!("[perdir] WARNING: home directory access is denied by manifest policy (not enforced — requires OS-level sandboxing)");
        }
        PermissionMode::ReadOnly => {
            eprintln!("[perdir] NOTICE: home directory is read-only by manifest policy (not enforced — requires OS-level sandboxing)");
        }
        _ => {}
    }

    if !world.permissions.gpu {
        eprintln!("[perdir] NOTICE: GPU access is disabled by manifest policy (not enforced)");
    }

    true
}

fn apply_permission_env(root: &Path, world: &World, cmd: &mut Command) {
    if matches!(world.permissions.network, PermissionMode::Deny) {
        cmd.env("no_proxy", "*");
        cmd.env("NO_PROXY", "*");
        cmd.env_remove("http_proxy");
        cmd.env_remove("https_proxy");
        cmd.env_remove("HTTP_PROXY");
        cmd.env_remove("HTTPS_PROXY");
        sandbox::deny_network(cmd);
    }

    if matches!(world.permissions.home, PermissionMode::Deny) {
        let sandbox_home = root.join(PERDIR_DIR).join("home");
        let _ = fs::create_dir_all(&sandbox_home);
        cmd.env("HOME", sandbox_home.to_string_lossy().to_string());
    }
}

mod sandbox {
    use super::Command;
    use std::io;
    use std::os::unix::process::CommandExt;

    pub fn deny_network(cmd: &mut Command) {
        unsafe {
            cmd.pre_exec(|| {
                #[cfg(target_os = "macos")]
                {
                    apply_macos_sandbox()?;
                }
                #[cfg(target_os = "linux")]
                {
                    apply_linux_namespace()?;
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let _ = io::Error::new(
                        io::ErrorKind::Unsupported,
                        "network sandboxing not supported on this OS",
                    );
                }
                Ok(())
            });
        }
    }

    #[cfg(target_os = "macos")]
    unsafe fn apply_macos_sandbox() -> io::Result<()> {
        use std::ffi::CString;
        use std::os::raw::{c_char, c_int};

        extern "C" {
            fn sandbox_init(
                profile: *const c_char,
                flags: u64,
                errorbuf: *mut *mut c_char,
            ) -> c_int;
        }

        let profile = CString::new("kSBXProfileNoNetwork").unwrap();
        let mut err_buf: *mut c_char = std::ptr::null_mut();

        // 0x0001 = SANDBOX_NAMED (built-in profile)
        if sandbox_init(profile.as_ptr(), 0x0001, &mut err_buf) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "macOS seatbelt sandbox initialization failed",
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    unsafe fn apply_linux_namespace() -> io::Result<()> {
        if libc::unshare(libc::CLONE_NEWNET) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_escape_plain() {
        assert_eq!(shell_escape("hello"), "hello");
    }

    #[test]
    fn test_shell_escape_single_quote() {
        assert_eq!(shell_escape("it's"), "it'\\''s");
    }

    #[test]
    fn test_shell_escape_backslash() {
        assert_eq!(shell_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_shell_escape_combined() {
        assert_eq!(shell_escape("it's a\\test"), "it'\\''s a\\\\test");
    }

    #[test]
    fn test_find_world_root_found() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join(WORLD_FILE), "").unwrap();

        let sub = dir.path().join("nested").join("deep");
        fs::create_dir_all(&sub).unwrap();

        let root = find_world_root(sub).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn test_find_world_root_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_world_root(dir.path().to_path_buf());
        assert!(result.is_err());
    }

    #[test]
    fn test_append_log_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();

        append_log(dir.path(), "test-action").unwrap();
        append_log(dir.path(), "second-action").unwrap();

        let contents = fs::read_to_string(perdir.join(LOG_FILE)).unwrap();
        assert!(contents.contains("test-action"));
        assert!(contents.contains("second-action"));
    }

    #[test]
    fn test_world_serde_roundtrip() {
        let world = World {
            name: "test-project".to_string(),
            runtime: Runtime {
                python: Some("3.12".to_string()),
                packages: vec!["python".to_string(), "nodejs".to_string()],
                pip_packages: vec![],
                env: [("RUST_LOG".to_string(), "debug".to_string())]
                    .into_iter()
                    .collect(),
            },
            permissions: Permissions {
                network: PermissionMode::Deny,
                home: PermissionMode::Allow,
                gpu: true,
            },
            ai: Ai {
                context: vec!["README.md".to_string()],
                memory_file: ".perdir/memory.md".to_string(),
                model: "gpt-4".to_string(),
            },
        };

        let toml_str = toml::to_string_pretty(&world).unwrap();
        let parsed: World = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, world.name);
        assert_eq!(parsed.runtime.python, world.runtime.python);
        assert_eq!(parsed.runtime.packages, world.runtime.packages);
        assert_eq!(parsed.permissions.gpu, world.permissions.gpu);
        assert_eq!(parsed.ai.model, world.ai.model);
    }

    #[test]
    fn test_world_serde_permission_kebab_case() {
        let toml_str = r#"
name = "test"
runtime = { python = "3.12", packages = [], pip_packages = [], env = {} }
permissions = { network = "read-only", home = "allow", gpu = false }
ai = { context = [], memory_file = "mem.md", model = "test" }
"#;
        let world: World = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            world.permissions.network,
            PermissionMode::ReadOnly
        ));
        assert!(matches!(world.permissions.home, PermissionMode::Allow));
    }

    #[test]
    fn test_validate_world_clean() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join("memory.md"), "# Memory\n").unwrap();
        fs::write(dir.path().join("README.md"), "# Test\n").unwrap();

        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: Some("3.12".to_string()),
                packages: vec!["python".to_string()],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Ask,
                home: PermissionMode::ReadOnly,
                gpu: false,
            },
            ai: Ai {
                context: vec!["README.md".to_string()],
                memory_file: ".perdir/memory.md".to_string(),
                model: "local".to_string(),
            },
        };

        let issues = validate_world(dir.path(), &world);
        assert!(issues.is_empty(), "expected no issues, got: {:?}", issues);
    }

    #[test]
    fn test_validate_world_missing_context_path() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join("memory.md"), "# Memory\n").unwrap();

        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: Some("3.12".to_string()),
                packages: vec!["python".to_string()],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Ask,
                home: PermissionMode::ReadOnly,
                gpu: false,
            },
            ai: Ai {
                context: vec!["README.md".to_string()],
                memory_file: ".perdir/memory.md".to_string(),
                model: "local".to_string(),
            },
        };

        let issues = validate_world(dir.path(), &world);
        assert!(issues
            .iter()
            .any(|i| i.contains("README.md") && i.contains("does not exist")));
    }

    #[test]
    fn test_validate_world_empty_name() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join("memory.md"), "# Memory\n").unwrap();

        let world = World {
            name: "".to_string(),
            runtime: Runtime {
                python: Some("3.12".to_string()),
                packages: vec!["python".to_string()],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Ask,
                home: PermissionMode::ReadOnly,
                gpu: false,
            },
            ai: Ai {
                context: vec![],
                memory_file: ".perdir/memory.md".to_string(),
                model: "local".to_string(),
            },
        };

        let issues = validate_world(dir.path(), &world);
        assert!(issues.iter().any(|i| i.contains("name is empty")));
    }

    #[test]
    fn test_validate_world_bad_python_version() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join("memory.md"), "# Memory\n").unwrap();

        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: Some("latest".to_string()),
                packages: vec!["python".to_string()],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Ask,
                home: PermissionMode::ReadOnly,
                gpu: false,
            },
            ai: Ai {
                context: vec![],
                memory_file: ".perdir/memory.md".to_string(),
                model: "local".to_string(),
            },
        };

        let issues = validate_world(dir.path(), &world);
        assert!(issues
            .iter()
            .any(|i| i.contains("python version") && i.contains("unexpected")));
    }

    #[test]
    fn test_shell_init_output() {
        shell_init().unwrap();
    }

    #[test]
    fn test_activate_venv_no_python() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();

        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: None,
                packages: vec![],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Ask,
                home: PermissionMode::ReadOnly,
                gpu: false,
            },
            ai: Ai {
                context: vec![],
                memory_file: ".perdir/memory.md".to_string(),
                model: "local".to_string(),
            },
        };

        let result = activate_venv(dir.path(), &world);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_permissions_deny_network() {
        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: None,
                packages: vec![],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Deny,
                home: PermissionMode::Allow,
                gpu: true,
            },
            ai: Ai {
                context: vec![],
                memory_file: "mem.md".to_string(),
                model: "test".to_string(),
            },
        };
        assert!(check_permissions(&world));
    }

    #[test]
    fn test_check_permissions_all_allow() {
        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: None,
                packages: vec![],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Allow,
                home: PermissionMode::Allow,
                gpu: true,
            },
            ai: Ai {
                context: vec![],
                memory_file: "mem.md".to_string(),
                model: "test".to_string(),
            },
        };
        assert!(check_permissions(&world));
    }

    #[test]
    fn test_apply_permission_env_deny_network() {
        let world = World {
            name: "test".to_string(),
            runtime: Runtime {
                python: None,
                packages: vec![],
                pip_packages: vec![],
                env: Default::default(),
            },
            permissions: Permissions {
                network: PermissionMode::Deny,
                home: PermissionMode::Allow,
                gpu: true,
            },
            ai: Ai {
                context: vec![],
                memory_file: "mem.md".to_string(),
                model: "test".to_string(),
            },
        };

        let mut cmd = Command::new("echo");
        apply_permission_env(Path::new("/tmp"), &world, &mut cmd);
    }

    #[test]
    fn test_clean_removes_venv() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join(WORLD_FILE), "name = \"test\"\n").unwrap();

        let venv_dir = perdir.join("venv");
        fs::create_dir_all(venv_dir.join("bin")).unwrap();
        fs::write(venv_dir.join(".perdir_packages"), "[]").unwrap();

        assert!(venv_dir.exists());
        assert!(clean_venv(dir.path()).is_ok());
        assert!(!venv_dir.exists());
    }

    #[test]
    fn test_clean_no_venv() {
        let dir = tempfile::tempdir().unwrap();
        let perdir = dir.path().join(PERDIR_DIR);
        fs::create_dir_all(&perdir).unwrap();
        fs::write(perdir.join(WORLD_FILE), "name = \"test\"\n").unwrap();

        assert!(!perdir.join("venv").exists());
        assert!(clean_venv(dir.path()).is_ok());
    }

    #[test]
    fn test_extract_toml_block_found() {
        let content = "Here is the manifest:\n```toml\nname = \"test\"\n```\nDone.";
        let result = extract_toml_block(content).unwrap();
        assert_eq!(result, "name = \"test\"");
    }

    #[test]
    fn test_extract_toml_block_not_found() {
        let content = "No code block here.";
        assert!(extract_toml_block(content).is_none());
    }
}
