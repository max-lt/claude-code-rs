use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::permission::PermissionConfig;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenType {
    OAuthAccess,
    OAuthRefresh,
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub token: String,
    pub is_oauth: bool,
}

impl Credentials {
    pub fn token_type(&self) -> TokenType {
        if self.token.starts_with("sk-ant-oat") {
            TokenType::OAuthAccess
        } else if self.token.starts_with("sk-ant-ort") {
            TokenType::OAuthRefresh
        } else {
            TokenType::ApiKey
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("Could not determine config directory")?;
    let dir = base.join("claude-code-rs");

    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create config directory")?;
    }

    Ok(dir)
}

fn credentials_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials.json"))
}

pub fn load_credentials() -> Result<Option<Credentials>> {
    let path = credentials_path()?;

    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path).context("Failed to read credentials file")?;
    let creds: Credentials =
        serde_json::from_str(&contents).context("Failed to parse credentials file")?;
    Ok(Some(creds))
}

pub fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    let contents = serde_json::to_string_pretty(creds)?;
    fs::write(&path, &contents).context("Failed to write credentials file")?;

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms).context("Failed to set file permissions")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Settings (permissions, etc.)
// ---------------------------------------------------------------------------

/// Composable merge for layered configuration.
pub trait Mergeable {
    fn merge(self, other: Self) -> Self;
}

impl Mergeable for PermissionConfig {
    fn merge(mut self, other: Self) -> Self {
        self.allow.extend(other.allow);
        self.deny.extend(other.deny);
        self.additional_directories
            .extend(other.additional_directories);
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub permissions: PermissionConfig,
}

impl Mergeable for Settings {
    fn merge(self, other: Self) -> Self {
        Self {
            permissions: self.permissions.merge(other.permissions),
        }
    }
}

/// Load settings by merging three layers (rules from all files are combined):
///
/// 1. `~/.claude/settings.json` — global user settings
/// 2. `{project_dir}/.claude/settings.json` — project settings (committed)
/// 3. `{project_dir}/.claude/settings.local.json` — local overrides (gitignored)
pub fn load_settings(project_dir: &Path) -> Settings {
    let claude_dir = project_dir.join(".claude");

    let paths: Vec<PathBuf> = vec![
        dirs::home_dir().map(|h| h.join(".claude").join("settings.json")),
        Some(claude_dir.join("settings.json")),
        Some(claude_dir.join("settings.local.json")),
    ]
    .into_iter()
    .flatten()
    .collect();

    load_settings_from_paths(&paths)
}

/// Load and merge settings from an explicit list of file paths (in order).
/// Missing or malformed files are silently skipped.
pub fn load_settings_from_paths(paths: &[PathBuf]) -> Settings {
    paths
        .iter()
        .filter_map(|p| load_settings_file(p))
        .reduce(Mergeable::merge)
        .unwrap_or_default()
}

fn load_settings_file(path: &Path) -> Option<Settings> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::Tool;

    // -----------------------------------------------------------------------
    // Mergeable — PermissionConfig
    // -----------------------------------------------------------------------

    #[test]
    fn merge_two_empty_configs() {
        let merged = PermissionConfig::default().merge(PermissionConfig::default());

        assert!(merged.allow.is_empty());
        assert!(merged.deny.is_empty());
        assert!(merged.additional_directories.is_empty());
    }

    #[test]
    fn merge_empty_into_non_empty() {
        let base = PermissionConfig {
            allow: vec!["Bash(ls:*)".into()],
            deny: vec!["Bash(rm:*)".into()],
            additional_directories: vec![PathBuf::from("/a")],
        };

        let merged = base.merge(PermissionConfig::default());

        assert_eq!(merged.allow, vec!["Bash(ls:*)"]);
        assert_eq!(merged.deny, vec!["Bash(rm:*)"]);
        assert_eq!(merged.additional_directories, vec![PathBuf::from("/a")]);
    }

    #[test]
    fn merge_non_empty_into_empty() {
        let overlay = PermissionConfig {
            allow: vec!["Bash(ls:*)".into()],
            deny: vec!["Bash(rm:*)".into()],
            additional_directories: vec![PathBuf::from("/b")],
        };

        let merged = PermissionConfig::default().merge(overlay);

        assert_eq!(merged.allow, vec!["Bash(ls:*)"]);
        assert_eq!(merged.deny, vec!["Bash(rm:*)"]);
        assert_eq!(merged.additional_directories, vec![PathBuf::from("/b")]);
    }

    #[test]
    fn merge_combines_all_fields() {
        let a = PermissionConfig {
            allow: vec!["Bash(psql:*)".into()],
            deny: vec!["Bash(rm:*)".into()],
            additional_directories: vec![PathBuf::from("/a")],
        };
        let b = PermissionConfig {
            allow: vec!["Bash(find:*)".into()],
            deny: vec!["Bash(sudo:*)".into()],
            additional_directories: vec![PathBuf::from("/b")],
        };

        let merged = a.merge(b);

        assert_eq!(merged.allow, vec!["Bash(psql:*)", "Bash(find:*)"]);
        assert_eq!(merged.deny, vec!["Bash(rm:*)", "Bash(sudo:*)"]);
        assert_eq!(
            merged.additional_directories,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn merge_preserves_duplicates() {
        let a = PermissionConfig {
            allow: vec!["Bash(ls:*)".into()],
            ..Default::default()
        };
        let b = PermissionConfig {
            allow: vec!["Bash(ls:*)".into()],
            ..Default::default()
        };

        let merged = a.merge(b);

        // Duplicates are kept — harmless, and avoids the cost of dedup.
        assert_eq!(merged.allow.len(), 2);
    }

    #[test]
    fn merge_preserves_order_base_then_overlay() {
        let a = PermissionConfig {
            allow: vec!["Bash(a:*)".into(), "Bash(b:*)".into()],
            ..Default::default()
        };
        let b = PermissionConfig {
            allow: vec!["Bash(c:*)".into(), "Bash(d:*)".into()],
            ..Default::default()
        };

        let merged = a.merge(b);

        assert_eq!(
            merged.allow,
            vec!["Bash(a:*)", "Bash(b:*)", "Bash(c:*)", "Bash(d:*)"]
        );
    }

    // -----------------------------------------------------------------------
    // Mergeable — Settings (delegates to PermissionConfig)
    // -----------------------------------------------------------------------

    #[test]
    fn settings_merge_delegates_to_permission_config() {
        let a = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(psql:*)".into()],
                ..Default::default()
            },
        };
        let b = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(find:*)".into()],
                ..Default::default()
            },
        };

        let merged = a.merge(b);

        assert_eq!(
            merged.permissions.allow,
            vec!["Bash(psql:*)", "Bash(find:*)"]
        );
    }

    // -----------------------------------------------------------------------
    // Three-way merge (the real scenario: global → project → local)
    // -----------------------------------------------------------------------

    #[test]
    fn three_way_merge() {
        let global = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(git:*)".into()],
                deny: vec!["Bash(rm -rf:*)".into()],
                additional_directories: vec![PathBuf::from("/global/shared")],
            },
        };
        let project = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(cargo:*)".into()],
                additional_directories: vec![PathBuf::from("/project-extra")],
                ..Default::default()
            },
        };
        let local = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(psql:*)".into()],
                deny: vec!["Bash(sudo:*)".into()],
                ..Default::default()
            },
        };

        let merged = global.merge(project).merge(local);

        assert_eq!(
            merged.permissions.allow,
            vec!["Bash(git:*)", "Bash(cargo:*)", "Bash(psql:*)"]
        );
        assert_eq!(
            merged.permissions.deny,
            vec!["Bash(rm -rf:*)", "Bash(sudo:*)"]
        );
        assert_eq!(
            merged.permissions.additional_directories,
            vec![
                PathBuf::from("/global/shared"),
                PathBuf::from("/project-extra")
            ]
        );
    }

    // -----------------------------------------------------------------------
    // Cross-layer deny overrides allow (integration with PermissionConfig::check)
    // -----------------------------------------------------------------------

    #[test]
    fn deny_from_local_blocks_allow_from_global() {
        let global = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(*)".into()],
                ..Default::default()
            },
        };
        let local = Settings {
            permissions: PermissionConfig {
                deny: vec!["Bash(rm:*)".into()],
                ..Default::default()
            },
        };

        let merged = global.merge(local);
        let project = Path::new("/project");

        assert_eq!(
            merged
                .permissions
                .check(&Tool::Bash { command: "ls -la" }, project),
            Some(true)
        );
        assert_eq!(
            merged.permissions.check(
                &Tool::Bash {
                    command: "rm -rf /"
                },
                project
            ),
            Some(false)
        );
    }

    #[test]
    fn deny_from_project_blocks_allow_from_local() {
        let project_settings = Settings {
            permissions: PermissionConfig {
                deny: vec!["Bash(curl:*)".into()],
                ..Default::default()
            },
        };
        let local = Settings {
            permissions: PermissionConfig {
                allow: vec!["Bash(curl:*)".into()],
                ..Default::default()
            },
        };

        let merged = project_settings.merge(local);
        let project = Path::new("/project");

        // Deny always wins, even if allow exists for the same pattern.
        assert_eq!(
            merged.permissions.check(
                &Tool::Bash {
                    command: "curl http://example.com"
                },
                project
            ),
            Some(false)
        );
    }

    // -----------------------------------------------------------------------
    // Additional directories from multiple layers
    // -----------------------------------------------------------------------

    #[test]
    fn additional_directories_merged_across_layers() {
        let global = Settings {
            permissions: PermissionConfig {
                additional_directories: vec![PathBuf::from("/shared/libs")],
                ..Default::default()
            },
        };
        let local = Settings {
            permissions: PermissionConfig {
                additional_directories: vec![PathBuf::from("/Users/max/other-project")],
                ..Default::default()
            },
        };

        let merged = global.merge(local);
        let project = Path::new("/project");

        assert_eq!(
            merged.permissions.check(
                &Tool::FileRead {
                    path: Path::new("/shared/libs/util.rs")
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            merged.permissions.check(
                &Tool::FileWrite {
                    path: Path::new("/Users/max/other-project/main.rs")
                },
                project
            ),
            Some(true)
        );
        assert_eq!(
            merged.permissions.check(
                &Tool::FileRead {
                    path: Path::new("/etc/passwd")
                },
                project
            ),
            None
        );
    }

    // -----------------------------------------------------------------------
    // load_settings — filesystem integration tests
    // -----------------------------------------------------------------------

    /// Helper: build paths for project + local settings inside a temp dir.
    fn project_paths(claude_dir: &Path) -> Vec<PathBuf> {
        vec![
            claude_dir.join("settings.json"),
            claude_dir.join("settings.local.json"),
        ]
    }

    #[test]
    fn load_settings_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = project_paths(&tmp.path().join(".claude"));

        let s = load_settings_from_paths(&paths);

        assert!(s.permissions.allow.is_empty());
        assert!(s.permissions.deny.is_empty());
        assert!(s.permissions.additional_directories.is_empty());
    }

    #[test]
    fn load_settings_project_json_only() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(
            claude_dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(cargo:*)"]}}"#,
        )
        .unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert_eq!(s.permissions.allow, vec!["Bash(cargo:*)"]);
        assert!(s.permissions.deny.is_empty());
    }

    #[test]
    fn load_settings_local_json_only() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(
            claude_dir.join("settings.local.json"),
            r#"{"permissions":{"allow":["Bash(psql:*)"],"additionalDirectories":["/extra"]}}"#,
        )
        .unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert_eq!(s.permissions.allow, vec!["Bash(psql:*)"]);
        assert_eq!(
            s.permissions.additional_directories,
            vec![PathBuf::from("/extra")]
        );
    }

    #[test]
    fn load_settings_merges_project_and_local() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(
            claude_dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(cargo:*)"]}}"#,
        )
        .unwrap();
        fs::write(
            claude_dir.join("settings.local.json"),
            r#"{"permissions":{"allow":["Bash(psql:*)"],"deny":["Bash(rm:*)"]}}"#,
        )
        .unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert_eq!(s.permissions.allow, vec!["Bash(cargo:*)", "Bash(psql:*)"]);
        assert_eq!(s.permissions.deny, vec!["Bash(rm:*)"]);
    }

    #[test]
    fn load_settings_malformed_json_is_silently_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(claude_dir.join("settings.json"), "not json!!!").unwrap();
        fs::write(
            claude_dir.join("settings.local.json"),
            r#"{"permissions":{"allow":["Bash(psql:*)"]}}"#,
        )
        .unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        // Broken file skipped, valid file still loaded.
        assert_eq!(s.permissions.allow, vec!["Bash(psql:*)"]);
    }

    #[test]
    fn load_settings_empty_json_object() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(claude_dir.join("settings.json"), "{}").unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert!(s.permissions.allow.is_empty());
        assert!(s.permissions.deny.is_empty());
        assert!(s.permissions.additional_directories.is_empty());
    }

    #[test]
    fn load_settings_partial_json_uses_defaults_for_missing_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Has allow but no deny, no additionalDirectories
        fs::write(
            claude_dir.join("settings.json"),
            r#"{"permissions":{"allow":["Bash(ls:*)"]}}"#,
        )
        .unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert_eq!(s.permissions.allow, vec!["Bash(ls:*)"]);
        assert!(s.permissions.deny.is_empty());
        assert!(s.permissions.additional_directories.is_empty());
    }

    #[test]
    fn load_settings_empty_string_file_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(claude_dir.join("settings.json"), "").unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert!(s.permissions.allow.is_empty());
    }

    #[test]
    fn load_settings_both_malformed_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        fs::write(claude_dir.join("settings.json"), "{{{bad").unwrap();
        fs::write(claude_dir.join("settings.local.json"), "also broken").unwrap();

        let s = load_settings_from_paths(&project_paths(&claude_dir));

        assert!(s.permissions.allow.is_empty());
        assert!(s.permissions.deny.is_empty());
        assert!(s.permissions.additional_directories.is_empty());
    }

    // -----------------------------------------------------------------------
    // Full integration: load from disk → merged config → permission checks
    // -----------------------------------------------------------------------

    #[test]
    fn full_integration_real_world_scenario() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Project settings: committed to the repo
        fs::write(
            claude_dir.join("settings.json"),
            r#"{
                "permissions": {
                    "allow": ["Bash(cargo:*)", "Bash(git:*)"],
                    "deny": ["Bash(cargo publish:*)"]
                }
            }"#,
        )
        .unwrap();

        // Local settings: user-specific, gitignored
        fs::write(
            claude_dir.join("settings.local.json"),
            r#"{
                "permissions": {
                    "allow": [
                        "Bash(psql:*)",
                        "Bash(bun scripts/generate-types.ts:*)",
                        "Bash(bun run generate-types:*)",
                        "Bash(find:*)"
                    ],
                    "additionalDirectories": [
                        "/Users/max/Documents/workspaces/OPENWORKERS/openworkers-dash/",
                        "/Users/max/Documents/workspaces/OPENWORKERS"
                    ]
                }
            }"#,
        )
        .unwrap();

        let settings = load_settings(tmp.path());
        let project_dir = tmp.path();

        // From project settings.json
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "cargo build"
                },
                project_dir
            ),
            Some(true)
        );
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "git status"
                },
                project_dir
            ),
            Some(true)
        );

        // Denied by project (even though cargo:* is allowed)
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "cargo publish"
                },
                project_dir
            ),
            Some(false)
        );

        // From settings.local.json
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "psql -U admin mydb"
                },
                project_dir
            ),
            Some(true)
        );
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "find . -name '*.rs'"
                },
                project_dir
            ),
            Some(true)
        );
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "bun scripts/generate-types.ts"
                },
                project_dir
            ),
            Some(true)
        );
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "bun run generate-types"
                },
                project_dir
            ),
            Some(true)
        );

        // No matching rule → should prompt (None)
        assert_eq!(
            settings.permissions.check(
                &Tool::Bash {
                    command: "curl http://evil.com"
                },
                project_dir
            ),
            None
        );

        // File in additional directory → allowed
        assert_eq!(
            settings.permissions.check(
                &Tool::FileRead {
                    path: Path::new(
                        "/Users/max/Documents/workspaces/OPENWORKERS/openworkers-dash/src/main.rs"
                    )
                },
                project_dir
            ),
            Some(true)
        );

        // File in project dir → allowed
        let project_file = tmp.path().join("src/lib.rs");
        assert_eq!(
            settings.permissions.check(
                &Tool::FileWrite {
                    path: &project_file
                },
                project_dir
            ),
            Some(true)
        );

        // File outside all allowed dirs → should prompt
        assert_eq!(
            settings.permissions.check(
                &Tool::FileRead {
                    path: Path::new("/etc/shadow")
                },
                project_dir
            ),
            None
        );
    }
}
