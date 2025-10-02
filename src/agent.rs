use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::debug;

use crate::{config, fsutil, llm};

const MAX_CONTEXT_BYTES_PER_FILE: usize = 8_000;

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub description: String,
    pub read: Option<String>,
    pub create: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ReadOutcome {
    Success { bytes: usize },
    Failed { error: String },
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ReadLog {
    pub path: String,
    pub outcome: ReadOutcome,
}

#[derive(Debug, Clone)]
pub enum CreateOutcome {
    Created,
    AlreadyExists,
    Failed { error: String },
}

#[derive(Debug, Clone)]
pub struct CreateLog {
    pub path: String,
    pub outcome: CreateOutcome,
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub plan: Vec<PlanStep>,
    pub reads: Vec<ReadLog>,
    pub creates: Vec<CreateLog>,
    pub response: llm::EditResponse,
}

pub async fn run(
    cfg: &config::AppConfig,
    repo_root: &Path,
    user_prompt: &str,
    mut base_context: String,
) -> Result<AgentOutcome> {
    let raw_plan = llm::generate_plan(cfg, user_prompt).await;
    let plan_steps = match raw_plan {
        Ok(text) => match parse_plan(&text) {
            Some(plan) if !plan.is_empty() => plan,
            _ => fallback_plan(user_prompt),
        },
        Err(err) => {
            debug!("plan generation failed: {err:?}");
            fallback_plan(user_prompt)
        }
    };

    let mut reads = Vec::new();
    let mut creates = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut seen_creations: HashSet<String> = HashSet::new();

    for step in &plan_steps {
        if let Some(path) = step
            .create
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if !seen_creations.insert(path.to_string()) {
                creates.push(CreateLog {
                    path: path.to_string(),
                    outcome: CreateOutcome::AlreadyExists,
                });
            } else {
                match create_file(repo_root, path) {
                    Ok(created) => {
                        creates.push(CreateLog {
                            path: path.to_string(),
                            outcome: if created {
                                CreateOutcome::Created
                            } else {
                                CreateOutcome::AlreadyExists
                            },
                        });
                    }
                    Err(err) => creates.push(CreateLog {
                        path: path.to_string(),
                        outcome: CreateOutcome::Failed {
                            error: err.to_string(),
                        },
                    }),
                }
            }
        }

        if let Some(path) = step
            .read
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            if !seen_paths.insert(path.to_string()) {
                reads.push(ReadLog {
                    path: path.to_string(),
                    outcome: ReadOutcome::Skipped,
                });
                continue;
            }

            match read_file(repo_root, path) {
                Ok((abs, contents)) => {
                    let truncated = truncate(&contents, MAX_CONTEXT_BYTES_PER_FILE);
                    base_context.push_str(&format!("\n\n# File: {}\n{}", path, truncated));
                    reads.push(ReadLog {
                        path: path.to_string(),
                        outcome: ReadOutcome::Success {
                            bytes: fs::metadata(&abs)
                                .map(|m| m.len() as usize)
                                .unwrap_or(contents.len()),
                        },
                    });
                }
                Err(err) => {
                    reads.push(ReadLog {
                        path: path.to_string(),
                        outcome: ReadOutcome::Failed {
                            error: err.to_string(),
                        },
                    });
                }
            }
        }
    }

    let response = llm::propose_edits(cfg, user_prompt, &base_context).await?;

    Ok(AgentOutcome {
        plan: plan_steps,
        reads,
        creates,
        response,
    })
}

fn read_file(repo_root: &Path, rel: &str) -> Result<(PathBuf, String)> {
    let rel_path = Path::new(rel);
    let abs = fsutil::ensure_inside_repo(repo_root, rel_path)
        .with_context(|| format!("invalid path {rel}"))?;
    let contents =
        fs::read_to_string(&abs).with_context(|| format!("failed to read {}", abs.display()))?;
    Ok((abs, contents))
}

fn create_file(repo_root: &Path, rel: &str) -> Result<bool> {
    let rel_path = Path::new(rel);
    let abs = fsutil::ensure_inside_repo(repo_root, rel_path)
        .with_context(|| format!("invalid path {rel}"))?;
    if abs.exists() {
        return Ok(false);
    }
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dirs for {}", abs.display()))?;
    }
    fs::write(&abs, b"").with_context(|| format!("failed to create file {}", abs.display()))?;
    Ok(true)
}

fn parse_plan(content: &str) -> Option<Vec<PlanStep>> {
    #[derive(Deserialize)]
    struct RawPlan {
        plan: Vec<RawStep>,
    }

    #[derive(Deserialize)]
    struct RawStep {
        description: String,
        #[serde(default)]
        read: Option<String>,
        #[serde(default)]
        create: Option<String>,
    }

    let parsed: RawPlan = serde_json::from_str(content).ok()?;
    let steps = parsed
        .plan
        .into_iter()
        .filter_map(|step| {
            let desc = step.description.trim();
            if desc.is_empty() {
                None
            } else {
                Some(PlanStep {
                    description: desc.to_string(),
                    read: step
                        .read
                        .map(|r| r.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    create: step
                        .create
                        .map(|c| c.trim().to_string())
                        .filter(|s| !s.is_empty()),
                })
            }
        })
        .collect::<Vec<_>>();
    Some(steps)
}

fn fallback_plan(user_prompt: &str) -> Vec<PlanStep> {
    vec![PlanStep {
        description: format!("Review project context and answer: {user_prompt}"),
        read: None,
        create: None,
    }]
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }

    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max {
            break;
        }
        end = next;
    }

    s[..end].to_string()
}

pub fn format_read_log(log: &ReadLog) -> String {
    match &log.outcome {
        ReadOutcome::Success { bytes } => {
            format!("Read {} ({} bytes)", log.path, bytes)
        }
        ReadOutcome::Failed { error } => format!("Failed to read {}: {error}", log.path),
        ReadOutcome::Skipped => format!("Skipped duplicate read of {}", log.path),
    }
}

pub fn format_create_log(log: &CreateLog) -> String {
    match &log.outcome {
        CreateOutcome::Created => format!("Created {}", log.path),
        CreateOutcome::AlreadyExists => format!("Skipped create (exists) {}", log.path),
        CreateOutcome::Failed { error } => format!("Failed to create {}: {error}", log.path),
    }
}

pub fn summarize_turn(user_prompt: &str, outcome: &AgentOutcome) -> String {
    let mut summary = String::new();
    summary.push_str("User: ");
    summary.push_str(user_prompt);
    summary.push('\n');

    if outcome.plan.is_empty() {
        summary.push_str("Plan: (none)\n");
    } else {
        summary.push_str("Plan:\n");
        for (idx, step) in outcome.plan.iter().enumerate() {
            if let Some(path) = &step.read {
                summary.push_str(&format!(
                    "  {}. {} [read {}]\n",
                    idx + 1,
                    step.description,
                    path
                ));
            } else {
                summary.push_str(&format!("  {}. {}\n", idx + 1, step.description));
            }
        }
    }

    if outcome.reads.is_empty() {
        summary.push_str("Reads: (none)\n");
    } else {
        summary.push_str("Reads:\n");
        for log in &outcome.reads {
            summary.push_str("  ");
            summary.push_str(&format_read_log(log));
            summary.push('\n');
        }
    }

    if outcome.creates.is_empty() {
        summary.push_str("Creates: (none)\n");
    } else {
        summary.push_str("Creates:\n");
        for log in &outcome.creates {
            summary.push_str("  ");
            summary.push_str(&format_create_log(log));
            summary.push('\n');
        }
    }

    summary.push_str("Assistant:\n");
    let truncated = truncate(&outcome.response.content, 1_000);
    summary.push_str(&truncated);

    summary
}
