Here’s a polished AGENT.md you can drop into your repo — it documents exactly how Smol CLI works, so Codex (or anyone reading the repo) has a blueprint.

⸻

AGENT.md

Overview

Smol CLI is a lightweight coding agent for the terminal.
It connects to OpenRouter to process natural language requests, then applies safe, minimal file edits directly in your project.

Unlike heavy autonomous agents, Smol CLI is:
	•	Minimal → one binary, one chat command
	•	Safe → always shows diffs before applying edits
	•	Undoable → keeps backups for quick rollback

⸻

v0 Capabilities
	•	Chat REPL: smol chat
	•	Type requests like:
Change the CSS in web/index.html to make buttons rounded and blue.
	•	Edit Proposals: the model returns JSON with edits (path, op, anchor, snippet).
	•	Diff Preview: Smol shows a unified diff of changes before applying.
	•	Approval: you confirm (y/N) for each file.
	•	Backups + Undo: every edit is backed up in .smol/backups/<timestamp>/, and /undo reverts the last change.
	•	Slash commands inside chat:
	•	/login – set API key
	•	/model – change model (e.g. grok-4-fast:free)
	•	/clear – clear session history
	•	/undo – revert the last applied change
	•	/stats – show token/message stats

⸻

Edit Schema

Smol expects edits in a strict JSON format:

{
  "edits": [
    {
      "path": "folder/index.html",
      "op": "replace",           // "replace" | "insert_after" | "insert_before"
      "anchor": "<button class=\"btn\">",
      "snippet": "<button class=\"btn rounded bg-blue-600\">",
      "limit": 1,
      "rationale": "Round corners and add blue background"
    }
  ]
}

Rules:
	•	Use anchor-based edits (no whole-file rewrites).
	•	limit defaults to 1.
	•	op must be replace, insert_after, or insert_before.
	•	If the anchor is missing or matches too many times, Smol skips the edit.

⸻

Safety Model
	•	Path safety: edits must stay inside repo root.
	•	Max size: files ≤ 256 KB.
	•	Atomic writes: edits are written via temp files then swapped in.
	•	Backups: originals saved to .smol/backups/.
	•	Approval required: diffs are always shown, user must confirm.

⸻

Architecture

smol-cli/
  src/
    main.rs        # CLI entry (clap)
    chat.rs        # REPL + slash commands
    llm.rs         # OpenRouter client
    edits.rs       # schema + apply edits
    diff.rs        # unified diff generator
    config.rs      # load/save config
    fsutil.rs      # safe file I/O, backups

	•	llm.rs → sends chat/completions requests to OpenRouter.
	•	chat.rs → REPL loop, parses slash commands, dispatches to LLM or edit pipeline.
	•	edits.rs → validates & applies JSON edits.
	•	diff.rs → shows unified diffs with similar crate.
	•	fsutil.rs → ensures safe paths, backups, and atomic writes.

⸻

OpenRouter Integration
	•	Base URL: https://openrouter.ai/api/v1
	•	Default model: grok-4-fast:free
	•	Auth:
	•	Env var: OPENROUTER_API_KEY (or SMOL_API_KEY)
	•	Config file: ~/.config/smolcli/config.toml

Example config:

[provider]
base_url = "https://openrouter.ai/api/v1"
model    = "grok-4-fast:free"

[auth]
api_key = "sk-..."


⸻

Example Session

$ smol chat
> Change the CSS in web/index.html to make buttons rounded and blue.

— Proposed edits (1 file):
web/index.html
───────────────────────────────────────────────
@@
- <button class="btn">
+ <button class="btn rounded bg-blue-600">
───────────────────────────────────────────────
Reason: Round corners and add blue background

Apply this file? [y/N] y
Applied. Backup: .smol/backups/2025-10-01T20-17-22/web/index.html


⸻

Roadmap
	•	Support whole-file rewrites with safe review mode
	•	Add /models to list all OpenRouter models
	•	Add git safeguards (warn on dirty repo before edits)
	•	Add TUI mode with ratatui

⸻

👉 This file is your blueprint. It tells Codex and contributors exactly how Smol CLI v0 works and what constraints it enforces.
