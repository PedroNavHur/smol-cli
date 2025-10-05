                         ┌──────────────────────────────────────────────────┐
                         │                    Terminal                      │
                         │        Ratatui (UI) + Crossterm (events)         │
                         └──────────────┬───────────────────────────────────┘
                                        │ keypress, resize, draw frames
                                        ▼
┌───────────────────────────────────────────────────────────────────────────────────────────┐
│                                   Smol CLI (Binary)                                       │
│                                                                                           │
│  ┌─────────────── UI Layer ────────────────┐   ┌────────────── Core Engine ─────────────┐ │
│  │  • App State (Redux-ish)                │   │  • Command Router (/login /model …)    │ │
│  │  • Panels:                              │   │  • Session (chat history, counters)    │ │
│  │    - Chat Log                           │   │  • Prompt Builder (system/user)        │ │
│  │    - Diff Viewer (colored +/−)          │   │  • Model Curation (filter/dedupe)      │ │
│  │    - Actions/Help                       │   │  • Edit Engine (anchor-based apply)    │ │
│  │    - Input Box (tui-textarea)           │   │  • Safety Guards (path/size/limits)    │ │
│  │  • Keybinds: Y/N/U, Enter, Esc          │   │  • Backups/Undo (atomic write)         │ │
│  └─────────────────────────────────────────┘   │  • Diff Builder (unified)              │ │
│                                                │  • Logging/Tracing                     │ │
│  ┌─────────── Config & Storage ────────────┐   │  • Error Handling (anyhow)             │ │
│  │  • ~/.config/smolcli/config.toml        │   └────────────────────────────────────────┘ │
│  │  • .smol/backups/<ts>/…                 │                    │                         │
│  │  • .smol/cache/models.json              │                    │ requests/responses      │
│  │  • .smol/session.log                    │                    ▼                         │
│  └─────────────────────────────────────────┘        ┌───────────────────────────────────┐ │
│                                                     │    OpenRouter Client (reqwest)    │ │
│                                                     │  • /models (category=programming) │ │
│                                                     │  • /chat/completions              │ │
│                                                     │  • Bearer auth, retries, timeouts │ │
│                                                     └───────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────────────────────────────┘
