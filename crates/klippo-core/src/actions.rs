//! Klipper-style regex "Actions".
//!
//! An action matches clipboard text against a regex and, on a match, can run
//! one or more commands. Command templates use Klipper's placeholders:
//! `%s`/`%0` = whole match, `%1`..`%9` = capture groups, `%%` = literal `%`.
//!
//! Execution is **shell-free by default**: the template is split into argv
//! tokens *before* substitution, so a clipboard value containing `;`, `&&`,
//! spaces, etc. lands in a single argv slot and cannot inject extra commands.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

fn default_true() -> bool {
    true
}

/// A configured action: a regex plus the commands to offer on a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    pub regex: String,
    #[serde(default = "default_true")]
    pub strip_whitespace: bool,
    /// When true, the action menu pops up automatically on a match.
    #[serde(default)]
    pub automatic: bool,
    pub commands: Vec<ActionCommand>,
}

/// One command belonging to an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionCommand {
    /// Template with `%s`/`%0`..`%9` placeholders.
    pub command: String,
    #[serde(default)]
    pub output: OutputMode,
    /// Opt-in: run via `/bin/sh -c` (loses injection-safety — use sparingly).
    #[serde(default)]
    pub shell: bool,
}

/// What to do with a command's stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    /// Discard output (fire-and-forget, e.g. `xdg-open`).
    #[default]
    Ignore,
    /// Replace the clipboard with the command's stdout.
    ReplaceClipboard,
    /// Add the command's stdout as a new history entry.
    NewEntry,
}

/// A successful regex match: `groups[0]` is the whole match, `groups[1..]` the
/// capture groups (missing optional groups become empty strings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub groups: Vec<String>,
}

/// Test `text` against `action`. Returns the match groups when it matches.
pub fn match_action(action: &Action, text: &str) -> Result<Option<MatchResult>> {
    let subject = if action.strip_whitespace {
        text.trim()
    } else {
        text
    };
    let re = regex::Regex::new(&action.regex)?;
    Ok(re.captures(subject).map(|caps| {
        let groups = caps
            .iter()
            .map(|m| m.map(|mm| mm.as_str().to_string()).unwrap_or_default())
            .collect();
        MatchResult { groups }
    }))
}

/// A command resolved to a concrete program + argv, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub use_shell: bool,
    pub output: OutputMode,
}

/// Resolve `cmd`'s placeholders against a match into a spawnable command.
pub fn prepare(cmd: &ActionCommand, m: &MatchResult) -> Result<PreparedCommand> {
    if cmd.shell {
        let line = substitute(&cmd.command, &m.groups);
        return Ok(PreparedCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), line],
            use_shell: true,
            output: cmd.output,
        });
    }

    // Split FIRST, substitute per-token: clipboard content can never spill into
    // adjacent argv slots or introduce new tokens.
    let tokens = shell_words::split(&cmd.command).map_err(|e| Error::ActionParse(e.to_string()))?;
    let subbed: Vec<String> = tokens.iter().map(|t| substitute(t, &m.groups)).collect();
    let (program, args) = subbed
        .split_first()
        .ok_or_else(|| Error::ActionParse("empty command template".to_string()))?;
    Ok(PreparedCommand {
        program: program.clone(),
        args: args.to_vec(),
        use_shell: false,
        output: cmd.output,
    })
}

/// Substitute `%s`, `%0`..`%9` and `%%` in a single template string.
pub fn substitute(template: &str, groups: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                out.push('%');
                chars.next();
            }
            Some('s') => {
                out.push_str(groups.first().map(String::as_str).unwrap_or(""));
                chars.next();
            }
            Some(d) if d.is_ascii_digit() => {
                let idx = d.to_digit(10).unwrap() as usize;
                out.push_str(groups.get(idx).map(String::as_str).unwrap_or(""));
                chars.next();
            }
            _ => out.push('%'),
        }
    }
    out
}

/// Compiled detectors for the built-in "magic" actions.
static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^https?://\S+$").unwrap());
static EMAIL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap());

/// Built-in "magic" actions inferred from a clipboard value's apparent type
/// (Klipper's `EnableMagicMimeActions`). Returns only the actions whose
/// detector matches `text`, so callers can merge them with configured actions.
/// Each uses the same shell-free [`prepare`] path, so it stays injection-safe.
pub fn magic_actions(text: &str) -> Vec<Action> {
    let trimmed = text.trim();
    let mut out = Vec::new();
    if URL_RE.is_match(trimmed) {
        out.push(magic("Abrir link", r"^(https?://\S+)$", "xdg-open %s"));
    } else if EMAIL_RE.is_match(trimmed) {
        out.push(magic(
            "Enviar e-mail",
            r"^([^@\s]+@[^@\s]+\.[^@\s]+)$",
            "xdg-open mailto:%s",
        ));
    }
    if is_existing_local_path(trimmed) {
        out.push(magic("Abrir arquivo", r"^(.+)$", "xdg-open %s"));
    }
    out
}

/// Build a one-command magic action (fire-and-forget, shell-free).
fn magic(name: &str, regex: &str, command: &str) -> Action {
    Action {
        name: name.to_string(),
        regex: regex.to_string(),
        strip_whitespace: true,
        automatic: false,
        commands: vec![ActionCommand {
            command: command.to_string(),
            output: OutputMode::Ignore,
            shell: false,
        }],
    }
}

/// True when `text` is a single-line absolute (or `~/`) path that exists.
fn is_existing_local_path(text: &str) -> bool {
    if text.contains(['\n', '\r']) {
        return false;
    }
    let path = if let Some(rest) = text.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(home) => std::path::PathBuf::from(home).join(rest),
            None => return false,
        }
    } else if text.starts_with('/') {
        std::path::PathBuf::from(text)
    } else {
        return false;
    };
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_action() -> Action {
        Action {
            name: "Abrir URL".into(),
            regex: r"^(https?://\S+)$".into(),
            strip_whitespace: true,
            automatic: false,
            commands: vec![ActionCommand {
                command: "xdg-open %s".into(),
                output: OutputMode::Ignore,
                shell: false,
            }],
        }
    }

    #[test]
    fn substitute_handles_placeholders() {
        let g = vec!["https://x".to_string(), "https://x".to_string()];
        assert_eq!(substitute("open %s", &g), "open https://x");
        assert_eq!(substitute("g %1 %0", &g), "g https://x https://x");
        assert_eq!(substitute("100%% done", &g), "100% done");
        assert_eq!(substitute("missing %7", &g), "missing ");
    }

    #[test]
    fn match_trims_and_captures() {
        let m = match_action(&url_action(), "  https://example.com  ")
            .unwrap()
            .unwrap();
        assert_eq!(m.groups[0], "https://example.com");
        assert_eq!(m.groups[1], "https://example.com");
        assert!(match_action(&url_action(), "not a url").unwrap().is_none());
    }

    #[test]
    fn prepare_is_injection_safe() {
        let a = url_action();
        let m = MatchResult {
            groups: vec!["https://x; rm -rf ~".into()],
        };
        let p = prepare(&a.commands[0], &m).unwrap();
        assert_eq!(p.program, "xdg-open");
        // The dangerous value stays a single argv element — not re-tokenized.
        assert_eq!(p.args, vec!["https://x; rm -rf ~".to_string()]);
        assert!(!p.use_shell);
    }

    #[test]
    fn magic_actions_detect_url_and_email() {
        let urls = magic_actions("https://example.com");
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].name, "Abrir link");

        let mail = magic_actions("  me@example.com  ");
        assert_eq!(mail.len(), 1);
        assert_eq!(mail[0].name, "Enviar e-mail");

        assert!(magic_actions("just some text").is_empty());
        assert!(magic_actions("/nonexistent/path/klippo-xyz").is_empty());
    }

    #[test]
    fn magic_email_builds_mailto_argv() {
        let a = &magic_actions("me@example.com")[0];
        let m = match_action(a, "me@example.com").unwrap().unwrap();
        let p = prepare(&a.commands[0], &m).unwrap();
        assert_eq!(p.program, "xdg-open");
        assert_eq!(p.args, vec!["mailto:me@example.com".to_string()]);
    }
}
