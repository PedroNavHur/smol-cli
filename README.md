# Smol CLI

A lightweight, conservative coding agent for your terminal. Smol CLI helps you make safe, minimal edits to your codebase through an intuitive TUI chat interface.

## Features

- **Safe & Conservative**: Only proposes small, anchor-based file edits with clear rationales
- **Terminal UI**: Clean, distraction-free interface for reviewing and applying changes
- **No Shell Commands**: Focuses on code modifications, never executes system commands
- **Context Aware**: Analyzes your codebase and provides relevant suggestions
- **Review Before Apply**: Always shows diffs and requires explicit approval
- **Multi-Model Support**: Works with various AI models via OpenRouter

## Installation

### Prerequisites

- Rust 1.70+ ([install Rust](https://rustup.rs/))
- An OpenRouter API key ([get one here](https://openrouter.ai/))

### Build from Source

```bash
# Clone the repository
git clone https://github.com/sst/smol-cli.git
cd smol-cli

# Build the project
cargo build --release

# Set your API key
export OPENROUTER_API_KEY="sk-or-v1-..."

# Run the chat interface
cargo run --release -- chat
```

## Quick Start

1. **Start the chat**: `cargo run -- chat`
2. **Ask questions**: "What does this function do?" or "Tell me about this project"
3. **Make changes**: "Add error handling to the login function"
4. **Review diffs**: Use `y` to apply, `n` to skip, `b` to cancel
5. **Navigate**: `Ctrl+U/D` to scroll, `Ctrl+C` to quit

## Usage Examples

### Code Analysis
```
> What does the main function do?
> Tell me about this project's architecture
> Show me all the error handling patterns
```

### Code Modifications
```
> Add input validation to the user registration form
> Refactor the database queries to use async/await
> Update the API endpoints to return JSON responses
> Add logging to the payment processing function
```

### File Operations
```
> Create a new test file for the user service
> Add a README to the utils directory
> Rename the config file to config.yaml
```

## Configuration

### Environment Variables

- `OPENROUTER_API_KEY`: Your OpenRouter API key (required)
- `OPENROUTER_BASE_URL`: API base URL (default: https://openrouter.ai/api/v1)

### Model Selection

Use `/model` in the chat interface to see available models:

```
/model                    # List available models
/model gpt-4o-mini        # Switch to a specific model
/model 1                  # Select model by number
```

## Key Bindings

- `Enter`: Send message
- `Ctrl+U/D`: Scroll activity window
- `Ctrl+PageUp/Down`: Page scroll
- `Ctrl+Home/End`: Jump to top/bottom
- `Tab`: Accept suggestion
- `Ctrl+Shift/Alt+Enter`: Insert newline
- `y/n/b`: Review actions (apply/skip/cancel)
- `Ctrl+C`: Quit

## Commands

- `/help`: Show available commands
- `/model`: Manage AI models
- `/clear`: Clear chat history
- `/stats`: Show usage statistics
- `/undo`: Undo last applied change
- `/quit`: Exit the application

## Safety & Philosophy

Smol CLI is designed with safety as the highest priority:

- **Minimal Changes**: Only suggests small, targeted edits
- **Clear Rationale**: Every change includes an explanation
- **Human Review**: All changes require explicit approval
- **No Destructive Operations**: Never deletes files or runs commands
- **Anchor-Based**: Uses unique code anchors to prevent incorrect matches

## Architecture

Smol CLI consists of:

- **Agent Core**: Planning and execution engine
- **LLM Integration**: OpenRouter API client with tool calling
- **TUI Interface**: Terminal-based chat and review interface
- **Diff Engine**: Unified diff generation and application
- **File System Utils**: Safe file operations within the repository

## Contributing

We welcome contributions! Please:

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## License

MIT License - see LICENSE file for details.

## Support

- [GitHub Issues](https://github.com/sst/smol-cli/issues)
---

**Smol CLI** - Making code changes small, safe, and simple.
