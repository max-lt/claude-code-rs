# ccrs

A lightweight reimplementation of [Claude Code](https://github.com/anthropics/claude-code) in Rust.

## Why?

`ccrs` is designed for performance and efficiency. While the official Claude Code CLI is a powerful tool, it requires approximately 175 MB of disk space and 500 MB – 3 GB+ of RAM during operation due to its Node.js/Bun architecture.

`ccrs` provides the same functionality in a **~23 MB** binary using **~10 MB RAM**, making it ideal for resource-constrained environments or users who prefer minimal overhead.

## Features

- **Full tool suite** — Bash, Git, Read, Write, Edit, Glob, Grep, Search
- **Agentic loop** — tool_use → permission check → execute → send result → continue
- **Interactive permissions** — colored prompts with rule-based auto-allow
- **Smart Git integration** — read-only commands (status, log, diff) auto-approved, write operations require permission
- **Settings compatibility** — reads `.claude/settings.json` and `.claude/settings.local.json` (same format as Claude Code)
- **OAuth PKCE** — same auth flow as the official CLI, with refresh token rotation
- **API key authentication**
- **Streaming responses** via SSE
- **Slash commands** — `/help`, `/quit`, `/clear`, `/model`

## Install

```
cargo install --path crates/cli
```

## Usage

```
ccrs
```

On first launch, authenticate via **OAuth** (browser) or **API key** (`sk-ant-...`).

Then type your messages at the `>` prompt. Claude streams responses and uses tools as needed, asking permission before executing.

### Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `/help` | `/h` | Show available commands |
| `/quit` | `/q` `/exit` | Exit |
| `/clear` | | Clear conversation history |
| `/model` | | List available models |
| `/model <name>` | | Switch model (e.g. `/model opus`) |

### Permissions

Create `.claude/settings.local.json` in your project:

```json
{
  "permissions": {
    "allow": [
      "Bash(cargo:*)", 
      "Bash(git:*)",
      "Git(commit:*)",
      "Git(push:*)"
    ],
    "deny": ["Bash(rm -rf:*)"],
    "additionalDirectories": ["/path/to/other/project"]
  }
}
```

**Auto-approved tools:**
- `Glob`, `Grep`, `Search`, `List` — always allowed
- `Read`, `Write`, `Edit` — auto-allowed in project directory
- `Git status`, `Git log`, `Git diff`, `Git show`, `Git blame`, `Git branch` — read-only git commands

**Require permission:**
- `Bash` commands (unless explicitly allowed)
- `Git commit`, `Git push`, `Git reset`, `Git checkout`, `Git add`, etc. — write operations

Three layers, merged in order:

1. `~/.claude/settings.json` — global
2. `.claude/settings.json` — project (committed)
3. `.claude/settings.local.json` — local (gitignored)

## Architecture

```
crates/
  core/   API client, streaming, tools (Bash/Git/Read/Write/Edit/Glob/Grep/Search), permissions, auth
  cli/    Terminal UI, interactive permissions, slash commands
  search/ Tantivy-based semantic search
```

## Credentials

Stored in `~/.config/claude-code-rs/credentials.json` (mode `0600`). Delete to re-authenticate.
