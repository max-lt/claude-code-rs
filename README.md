# claude-code-rs

A lightweight reimplementation of [Claude Code](https://github.com/anthropics/claude-code) in Rust.

## Why?

The official Claude Code CLI weighs **175 MB** on disk and routinely consumes **500 MB – 3 GB+ RAM** per session, with [documented memory leaks reaching 12–93 GB](https://github.com/anthropics/claude-code/issues/22188) and [50–80% idle CPU](https://github.com/anthropics/claude-code/issues/22275). This is a Node.js/V8 architecture problem, not a bug.

`ccrs` does the same job in a **4 MB** static binary using **~10 MB RAM**.

## Features

- **Tool use** — Claude can execute bash commands, read and write files via the API tool_use protocol
- **Agentic loop** — automatic tool_use → permission check → execute → send result → continue
- **Interactive permissions** — colored prompts before each tool execution, with rule-based auto-allow
- **Settings compatibility** — reads `.claude/settings.json` and `.claude/settings.local.json` (same format as Claude Code)
- **OAuth PKCE authentication** — same flow as the official CLI, with refresh token rotation
- **API key authentication**
- **Streaming responses** via SSE
- **Multi-turn conversation** with context bootstrap
- **Slash commands** — `/help`, `/quit`, `/clear`, `/model`
- **Model switching** — `/model opus`, `/model haiku`, `/model sonnet`

## Install

```
cargo install --path crates/cli
```

Binary name: `ccrs`

## Usage

```
ccrs
```

On first launch, you'll be prompted to authenticate:

- **OAuth** — opens your browser for login, paste the callback URL or code
- **API key** — paste your `sk-ant-...` key directly

Once authenticated, type your messages at the `>` prompt. Responses stream in real time. Claude will use tools (bash, file_read, file_write) as needed and ask for permission before executing.

### Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `/help` | `/h` | Show available commands |
| `/quit` | `/q` `/exit` | Exit the session |
| `/clear` | | Clear conversation history |
| `/model` | | List available models |
| `/model <name>` | | Switch model (e.g. `/model opus`) |

### Permission rules

Create `.claude/settings.local.json` in your project to auto-allow specific tools:

```json
{
  "permissions": {
    "allow": [
      "Bash(cargo:*)",
      "Bash(git:*)"
    ],
    "deny": [
      "Bash(rm -rf:*)"
    ],
    "additionalDirectories": [
      "/path/to/other/project"
    ]
  }
}
```

Rules are loaded from three layers (merged):
1. `~/.claude/settings.json` — global
2. `.claude/settings.json` — project (committed)
3. `.claude/settings.local.json` — local (gitignored)

## Architecture

```
crates/
  core/       — API client, streaming, tool system, permissions, auth
  cli/        — Terminal UI, interactive permissions, slash commands
```

## Credentials

Stored in `~/.config/claude-code-rs/credentials.json` (mode `0600`). Delete to re-authenticate.
