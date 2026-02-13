# ccrs

A lightweight reimplementation of [Claude Code](https://github.com/anthropics/claude-code) in Rust.

## Why?

The official Claude Code CLI weighs **175 MB** on disk and routinely consumes **500 MB – 3 GB+ RAM** per session, with [memory leaks reaching 12–93 GB](https://github.com/anthropics/claude-code/issues/22188) and [50–80% idle CPU](https://github.com/anthropics/claude-code/issues/22275). This is a Node.js/Bun architecture problem, not a bug.

`ccrs` does the same job in a **4 MB** static binary using **~10 MB RAM**.

## Features

- **Full tool suite** — Bash, Read, Write, Edit, Glob, Grep (same names and schemas as Claude Code)
- **Agentic loop** — tool_use → permission check → execute → send result → continue
- **Interactive permissions** — colored prompts with rule-based auto-allow
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
    "allow": ["Bash(cargo:*)", "Bash(git:*)"],
    "deny": ["Bash(rm -rf:*)"],
    "additionalDirectories": ["/path/to/other/project"]
  }
}
```

Three layers, merged in order:

1. `~/.claude/settings.json` — global
2. `.claude/settings.json` — project (committed)
3. `.claude/settings.local.json` — local (gitignored)

## Architecture

```
crates/
  core/   API client, streaming, tools (Bash/Read/Write/Edit/Glob/Grep), permissions, auth
  cli/    Terminal UI, interactive permissions, slash commands
```

## Credentials

Stored in `~/.config/claude-code-rs/credentials.json` (mode `0600`). Delete to re-authenticate.
