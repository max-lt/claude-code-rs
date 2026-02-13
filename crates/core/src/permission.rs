use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Describes a tool invocation that requires permission.
#[non_exhaustive]
pub enum Tool<'a> {
    Bash { command: &'a str },
    Read { path: &'a Path },
    Write { path: &'a Path },
    Edit { path: &'a Path },
    Git,
    Glob,
    Grep,
    Search,
}

/// Determines whether a given tool invocation is allowed.
///
/// `&mut self` allows stateful handlers (caching decisions, counters, etc.).
pub trait PermissionHandler: Send {
    fn allow(&mut self, tool: &Tool<'_>) -> bool;
}

/// Permits every tool invocation.
pub struct AllowAll;

impl PermissionHandler for AllowAll {
    fn allow(&mut self, _tool: &Tool<'_>) -> bool {
        true
    }
}

/// Denies every tool invocation.
pub struct DenyAll;

impl PermissionHandler for DenyAll {
    fn allow(&mut self, _tool: &Tool<'_>) -> bool {
        false
    }
}

impl PermissionHandler for Box<dyn PermissionHandler> {
    fn allow(&mut self, tool: &Tool<'_>) -> bool {
        (**self).allow(tool)
    }
}

// ---------------------------------------------------------------------------
// Rule-based permission configuration
// ---------------------------------------------------------------------------

/// Permission settings matching the Claude Code `.claude/settings.json` format.
///
/// ```json
/// {
///   "permissions": {
///     "allow": ["Bash(psql:*)", "Bash(find:*)"],
///     "deny": [],
///     "additionalDirectories": ["/extra/path"]
///   }
/// }
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PermissionConfig {
    #[serde(default)]
    pub allow: Vec<String>,

    #[serde(default)]
    pub deny: Vec<String>,

    #[serde(default, rename = "additionalDirectories")]
    pub additional_directories: Vec<PathBuf>,
}

impl PermissionConfig {
    /// Check if a tool invocation is auto-allowed by the configured rules.
    ///
    /// Returns `Some(true)` if explicitly allowed, `Some(false)` if explicitly
    /// denied, or `None` if no rule matches (caller should prompt).
    pub fn check(&self, tool: &Tool<'_>, project_dir: &Path) -> Option<bool> {
        // Deny rules take precedence
        if self.deny.iter().any(|r| rule_matches(r, tool)) {
            return Some(false);
        }

        // Check explicit allow rules
        if self.allow.iter().any(|r| rule_matches(r, tool)) {
            return Some(true);
        }

        // Read-only tools are always allowed
        match tool {
            Tool::Git | Tool::Glob | Tool::Grep | Tool::Search => return Some(true),
            _ => {}
        }

        // File operations in allowed directories are auto-allowed
        match tool {
            Tool::Read { path } | Tool::Write { path } | Tool::Edit { path } => {
                let resolved = resolve_path(path, project_dir);

                if resolved.starts_with(project_dir) {
                    return Some(true);
                }

                if self
                    .additional_directories
                    .iter()
                    .any(|dir| resolved.starts_with(dir))
                {
                    return Some(true);
                }
            }
            _ => {}
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Rule parsing and matching
// ---------------------------------------------------------------------------

/// Parse a rule string like `Bash(psql:*)` and check if it matches a tool.
fn rule_matches(rule: &str, tool: &Tool<'_>) -> bool {
    let Some((tool_name, pattern)) = parse_rule(rule) else {
        return false;
    };

    match (tool_name, tool) {
        ("Bash", Tool::Bash { command }) => pattern_matches(command, pattern),
        ("Read", Tool::Read { path }) => pattern_matches(&path.display().to_string(), pattern),
        ("Write", Tool::Write { path }) => pattern_matches(&path.display().to_string(), pattern),
        ("Edit", Tool::Edit { path }) => pattern_matches(&path.display().to_string(), pattern),
        _ => false,
    }
}

/// Extract tool name and pattern from `ToolName(pattern)`.
fn parse_rule(rule: &str) -> Option<(&str, &str)> {
    let open = rule.find('(')?;
    let close = rule.rfind(')')?;

    if close <= open {
        return None;
    }

    Some((&rule[..open], &rule[open + 1..close]))
}

/// Match a value against a pattern.
///
/// - `*` matches everything.
/// - `prefix:*` matches if `value` equals `prefix` or starts with `prefix `
///   (prefix followed by a space — i.e. the prefix is the command/path start).
/// - Anything else is an exact match.
fn pattern_matches(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix(":*") {
        return value == prefix
            || value.starts_with(prefix)
                && value
                    .as_bytes()
                    .get(prefix.len())
                    .is_some_and(|&b| b == b' ');
    }

    value == pattern
}

/// Resolve a potentially relative path against the project directory.
fn resolve_path(path: &Path, project_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rule() {
        assert_eq!(parse_rule("Bash(psql:*)"), Some(("Bash", "psql:*")));
        assert_eq!(parse_rule("Bash(*)"), Some(("Bash", "*")));
        assert_eq!(
            parse_rule("Bash(bun scripts/generate-types.ts:*)"),
            Some(("Bash", "bun scripts/generate-types.ts:*"))
        );
        assert_eq!(parse_rule("invalid"), None);
        assert_eq!(parse_rule("no_parens"), None);
    }

    #[test]
    fn test_pattern_matches_wildcard() {
        assert!(pattern_matches("anything", "*"));
        assert!(pattern_matches("", "*"));
    }

    #[test]
    fn test_pattern_matches_prefix() {
        assert!(pattern_matches("psql", "psql:*"));
        assert!(pattern_matches("psql -U admin mydb", "psql:*"));
        assert!(!pattern_matches("psql2", "psql:*"));
        assert!(!pattern_matches("xpsql", "psql:*"));
    }

    #[test]
    fn test_pattern_matches_multi_word_prefix() {
        let pat = "bun scripts/generate-types.ts:*";
        assert!(pattern_matches("bun scripts/generate-types.ts", pat));
        assert!(pattern_matches("bun scripts/generate-types.ts --flag", pat));
        assert!(!pattern_matches("bun scripts/generate-types.tsx", pat));
    }

    #[test]
    fn test_pattern_matches_exact() {
        assert!(pattern_matches("exact", "exact"));
        assert!(!pattern_matches("exact2", "exact"));
    }

    #[test]
    fn test_config_bash_rules() {
        let config = PermissionConfig {
            allow: vec!["Bash(psql:*)".to_string(), "Bash(find:*)".to_string()],
            ..Default::default()
        };

        let project = Path::new("/project");

        assert_eq!(
            config.check(
                &Tool::Bash {
                    command: "psql -U admin"
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            config.check(
                &Tool::Bash {
                    command: "find . -name '*.rs'"
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            config.check(
                &Tool::Bash {
                    command: "rm -rf /"
                },
                project
            ),
            None
        );
    }

    #[test]
    fn test_config_file_in_project_dir() {
        let config = PermissionConfig::default();
        let project = Path::new("/project");

        assert_eq!(
            config.check(
                &Tool::Read {
                    path: Path::new("/project/src/main.rs")
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            config.check(
                &Tool::Read {
                    path: Path::new("/other/secret.txt")
                },
                project
            ),
            None
        );
    }

    #[test]
    fn test_config_additional_directories() {
        let config = PermissionConfig {
            additional_directories: vec![PathBuf::from("/extra/allowed")],
            ..Default::default()
        };

        let project = Path::new("/project");

        assert_eq!(
            config.check(
                &Tool::Write {
                    path: Path::new("/extra/allowed/file.txt")
                },
                project
            ),
            Some(true)
        );
    }

    #[test]
    fn test_deny_overrides_allow() {
        let config = PermissionConfig {
            allow: vec!["Bash(*)".to_string()],
            deny: vec!["Bash(rm:*)".to_string()],
            ..Default::default()
        };

        let project = Path::new("/project");

        assert_eq!(
            config.check(&Tool::Bash { command: "ls" }, project),
            Some(true)
        );
        assert_eq!(
            config.check(
                &Tool::Bash {
                    command: "rm -rf /"
                },
                project
            ),
            Some(false)
        );
    }

    #[test]
    fn test_glob_grep_always_allowed() {
        let config = PermissionConfig::default();
        let project = Path::new("/project");

        assert_eq!(config.check(&Tool::Glob, project), Some(true));
        assert_eq!(config.check(&Tool::Grep, project), Some(true));
    }

    #[test]
    fn test_edit_in_project_dir() {
        let config = PermissionConfig::default();
        let project = Path::new("/project");

        assert_eq!(
            config.check(
                &Tool::Edit {
                    path: Path::new("/project/src/lib.rs")
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            config.check(
                &Tool::Edit {
                    path: Path::new("/other/file.rs")
                },
                project
            ),
            None
        );
    }
}
