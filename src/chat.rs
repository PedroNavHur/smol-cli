use crate::{config, diff as diffmod, edits, fsutil, llm};
use anyhow::{Context, Result};
use inquire::{Confirm, Password};
use regex::Regex;
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::debug;

pub async fn run(model_override: Option<String>) -> Result<()> {
    let mut cfg = config::load()?;
    if let Some(m) = model_override {
        cfg.provider.model = m;
    }

    // API key check or prompt via /login
    if cfg.auth.api_key.is_empty() {
        println!("No API key found. Use /login to set it (or set OPENROUTER_API_KEY).");
    }

    println!("Smol CLI — chat mode. Type /help for commands.");
    let mut history: Vec<String> = Vec::new();
    let mut last_backups: Vec<PathBuf> = Vec::new();

    loop {
        print!("> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        if input.starts_with('/') {
            match handle_slash(input, &mut cfg, &mut history, &mut last_backups).await? {
                Flow::Continue => continue,
                Flow::Exit => break,
            }
        } else {
            // Build a tiny context (v0): current README if exists
            let ctx = build_context()?;
            let resp = llm::propose_edits(&cfg, input, &ctx).await?;
            debug!("LLM raw: {resp}");

            match edits::parse_edits(&resp) {
                Ok(batch) => {
                    apply_with_review(batch, &mut last_backups)?;
                }
                Err(e) => {
                    println!("Model did not return valid edits JSON: {e}");
                    println!("Raw response:\n{resp}");
                }
            }

            history.push(input.to_string());
        }
    }

    Ok(())
}

enum Flow {
    Continue,
    Exit,
}

async fn handle_slash(
    input: &str,
    cfg: &mut config::AppConfig,
    history: &mut Vec<String>,
    last_backups: &mut Vec<PathBuf>,
) -> Result<Flow> {
    match input {
        "/help" => {
            println!("/login  /model  /clear  /undo  /stats  /quit");
        }
        "/quit" | "/exit" => return Ok(Flow::Exit),
        "/clear" => {
            history.clear();
            println!("History cleared.");
        }
        "/stats" => {
            println!("Messages: {}", history.len());
        }
        "/login" => {
            let key = Password::new("OpenRouter API key (sk-...):")
                .without_confirmation()
                .prompt()?;
            cfg.auth.api_key = key;
            config::save(cfg)?;
            println!("Saved API key to config.");
        }
        cmd if cmd.starts_with("/model") => {
            let parts: Vec<_> = cmd.split_whitespace().collect();
            if parts.len() == 2 {
                cfg.provider.model = parts[1].to_string();
                config::save(cfg)?;
                println!("Model set to {}", cfg.provider.model);
            } else {
                println!("Usage: /model <provider/model>, e.g., openai/gpt-4o-mini");
            }
        }
        "/undo" => {
            if let Some(b) = last_backups.pop() {
                if let Some(target) = target_from_backup(&b) {
                    if let Err(e) = fs::copy(&b, &target) {
                        println!("Undo failed: {e}");
                    } else {
                        println!("Reverted {}", target.display());
                    }
                } else {
                    println!("No target path found for backup {}", b.display());
                }
            } else {
                println!("Nothing to undo.");
            }
        }
        _ => println!("Unknown command. /help"),
    }
    Ok(Flow::Continue)
}

fn target_from_backup(backup_path: &PathBuf) -> Option<PathBuf> {
    // backup structure: .smol/backups/<ts>/RELATIVE/PATH
    // reconstruct target by removing ".smol/backups/<ts>/" prefix
    let smol = fsutil::smol_dir().ok()?;
    let backups = smol.join("backups");
    let rel = backup_path.strip_prefix(backups).ok()?;
    let comps: Vec<_> = rel.components().collect();
    if comps.len() >= 2 {
        // skip timestamp component
        let stripped: PathBuf = comps.iter().skip(1).collect();
        Some(std::env::current_dir().ok()?.join(stripped))
    } else {
        None
    }
}

fn build_context() -> Result<String> {
    let mut ctx = String::new();
    if let Ok(readme) = fs::read_to_string("README.md") {
        ctx.push_str("README.md:\n");
        ctx.push_str(&truncate(&readme, 10_000));
    }
    Ok(ctx)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}

fn timestamp_dir() -> Result<PathBuf> {
    let smol = fsutil::smol_dir()?;
    let backups = smol.join("backups");
    std::fs::create_dir_all(&backups).ok();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let p = backups.join(format!("{}", now));
    std::fs::create_dir_all(&p).ok();
    Ok(p)
}

fn is_write_blocked(path: &str) -> bool {
    // Cheap guard: block edits outside project or hidden/system paths
    let re = Regex::new(r#"(^/|^\.)"#).unwrap();
    re.is_match(path)
}

fn apply_with_review(batch: edits::EditBatch, last_backups: &mut Vec<PathBuf>) -> Result<()> {
    use std::path::Path;
    if batch.edits.is_empty() {
        println!("No edits proposed.");
        return Ok(());
    }

    let root = std::env::current_dir()?;
    let backup_root = timestamp_dir()?;

    for e in &batch.edits {
        if is_write_blocked(&e.path) {
            println!("Skipping suspicious path: {}", e.path);
            continue;
        }

        let abs = fsutil::ensure_inside_repo(&root, Path::new(&e.path))
            .with_context(|| format!("invalid path {}", e.path))?;

        let old =
            std::fs::read_to_string(&abs).with_context(|| format!("read {}", abs.display()))?;

        let new = match edits::apply_edit(&old, e) {
            Ok(n) => n,
            Err(err) => {
                println!("Skipping {}: {}", e.path, err);
                continue;
            }
        };

        if old == new {
            println!("No change for {}", e.path);
            continue;
        }

        // Show diff
        let udiff = diffmod::unified_diff(&old, &new, &e.path);
        println!("\n{}", "— Proposed edit —");
        println!("{}", e.path);
        println!("{}", "────────────────────────────────────────────────");
        println!("{}", udiff);
        if let Some(r) = &e.rationale {
            println!("Reason: {}", r);
        }

        // Confirm
        let yes = Confirm::new("Apply this file?")
            .with_default(false)
            .prompt()?;
        if yes {
            let backup_file = fsutil::backup_path(&backup_root, &abs, &root)?;
            fsutil::backup_and_write(&abs, &new, &backup_file)?;
            println!("Applied. Backup: {}", backup_file.display());
            last_backups.push(backup_file);
        } else {
            println!("Skipped {}", e.path);
        }
    }

    Ok(())
}
