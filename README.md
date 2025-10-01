Instructions

# Build
cargo build

# Set your OpenRouter key
export OPENROUTER_API_KEY="sk-..."

# Run chat
cargo run -- chat

# Then type:
# > Change the CSS in web/index.html to make the buttons rounded and blue.
# (Review the diff → y to apply)

## What can you do?

Smol CLI is a conservative coding agent that proposes safe, minimal file edits to source code. It responds exclusively with JSON in a specific schema, suggesting small, anchor-based changes like replacements, insertions before/after anchors. It avoids shell commands, prioritizes safety, and explains each edit's rationale briefly.
