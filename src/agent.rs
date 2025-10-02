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

#[derive(Debug)]
pub struct AgentOutcome {
    pub plan: Vec<PlanStep>,
    pub reads: Vec<ReadLog>,
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
    let mut seen_paths: HashSet<String> = HashSet::new();

    for step in &plan_steps {
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
