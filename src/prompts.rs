//! Pure prompt construction + response parsing.
//!
//! Everything in here is deterministic and side-effect free so it can be
//! exercised from unit tests without spawning subprocesses or the network.

pub const COMMIT_SYSTEM: &str = "You write Conventional Commit messages prefixed with a gitmoji.\n\
Format: <emoji> <type>(<optional scope>): <subject>\n\n\
Gitmoji → type mapping (pick the single best fit):\n\
- ✨ feat      — new feature\n\
- 🐛 fix       — bug fix\n\
- 📝 docs      — documentation only\n\
- 🎨 style     — formatting / whitespace / no code change\n\
- ♻️  refactor  — code change that neither fixes a bug nor adds a feature\n\
- ⚡️  perf      — performance improvement\n\
- ✅ test      — adding or fixing tests\n\
- 👷 build     — build system / dependencies\n\
- 💚 ci        — CI configuration\n\
- 🔧 chore     — tooling / maintenance\n\
- ⏪️  revert    — revert a previous commit\n\
- 🔥 (remove)  — removing code/files (use with refactor/chore)\n\
- 🔒 (security)— security fix (use with fix)\n\n\
Rules:\n\
- Start with exactly one gitmoji emoji, then a single space, then the conventional commit.\n\
- Subject: imperative mood, lowercase, no trailing period, <= 72 chars (including emoji).\n\
- Optionally add a blank line then a short body explaining the \"why\" (wrap at 72).\n\
- Output ONLY the commit message. No markdown, no code fences, no commentary.\n\n\
Examples:\n\
✨ feat(pr): detect and update existing PRs\n\
🐛 fix(commit): handle empty diff without panicking\n\
✅ test: cover dry-run and no-add flag paths";

pub const PR_SYSTEM: &str = "You write concise, helpful GitHub pull request descriptions.\n\
Output format (markdown):\n  \
<short imperative title on a single line, <= 72 chars, no trailing period>\n  \
<blank line>\n  \
## Summary\n  \
- bullet points of what changed and why\n  \
## Changes\n  \
- key file/area level changes\n  \
## Notes\n  \
- optional: testing, risks, follow-ups (omit section if nothing to say)\n\n\
Rules:\n\
- Title first line only. No \"#\", no quotes, no prefix like \"PR:\".\n\
- Then blank line, then body in markdown.\n\
- No code fences around the whole thing. No commentary.";

/// Truncate a diff with a visible marker. Returns (text, truncated?).
pub fn truncate_diff(diff: &str, max: usize) -> (String, bool) {
    if diff.len() > max {
        // find nearest char boundary <= max
        let mut cut = max;
        while !diff.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        (format!("{}\n...[truncated]", &diff[..cut]), true)
    } else {
        (diff.to_string(), false)
    }
}

pub fn commit_user_prompt(status: &str, diff: &str) -> String {
    format!("Files changed:\n{status}\n\nDiff:\n{diff}\n\nWrite the commit message.")
}

pub fn pr_user_prompt(
    branch: &str,
    base: &str,
    commits: &str,
    diff_stat: &str,
    diff: &str,
) -> String {
    format!(
        "Branch: {branch} -> {base}\n\nCommits:\n{commits}\n\nDiff stat:\n{diff_stat}\n\nDiff:\n{diff}\n\nWrite the PR title and body."
    )
}

/// Strip leading/trailing markdown code fences if the model wrapped its answer.
pub fn strip_fences(s: &str) -> String {
    let t = s.trim();
    let t = t
        .strip_prefix("```markdown")
        .or_else(|| t.strip_prefix("```md"))
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.trim_start_matches('\n');
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim().to_string()
}

/// Split a model response into (title, body). Title is the first non-empty line
/// with leading `#` markers stripped. Body is everything after, trimmed.
pub fn split_title_body(raw: &str) -> (String, String) {
    let cleaned = strip_fences(raw);
    let mut lines = cleaned.splitn(2, '\n');
    let title = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string();
    let body = lines.next().unwrap_or("").trim().to_string();
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_preserves_short_input() {
        let (out, truncated) = truncate_diff("hello", 100);
        assert_eq!(out, "hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_marks_long_input() {
        let long = "a".repeat(200);
        let (out, truncated) = truncate_diff(&long, 50);
        assert!(truncated);
        assert!(out.ends_with("...[truncated]"));
        assert!(out.starts_with(&"a".repeat(50)));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // 4-byte emoji at the cut point must not be sliced in half
        let s = format!("{}🎉ok", "x".repeat(20));
        let (out, _) = truncate_diff(&s, 22);
        // Should not panic; output should be valid UTF-8 by construction
        assert!(out.is_char_boundary(out.find("\n...").unwrap_or(out.len())));
    }

    #[test]
    fn strip_fences_removes_markdown_wrapper() {
        let raw = "```markdown\nhello\n```";
        assert_eq!(strip_fences(raw), "hello");
    }

    #[test]
    fn strip_fences_removes_plain_wrapper() {
        assert_eq!(strip_fences("```\nhi\n```"), "hi");
    }

    #[test]
    fn strip_fences_leaves_clean_text_alone() {
        assert_eq!(strip_fences("feat: add thing"), "feat: add thing");
    }

    #[test]
    fn split_title_body_simple() {
        let (t, b) = split_title_body("add feature\n\n## Summary\n- one");
        assert_eq!(t, "add feature");
        assert_eq!(b, "## Summary\n- one");
    }

    #[test]
    fn split_title_body_strips_hash_prefix() {
        let (t, _) = split_title_body("# add feature\n\nbody");
        assert_eq!(t, "add feature");
    }

    #[test]
    fn split_title_body_empty_body_is_ok() {
        let (t, b) = split_title_body("just a title");
        assert_eq!(t, "just a title");
        assert_eq!(b, "");
    }

    #[test]
    fn commit_system_prompt_requires_gitmoji() {
        // Spec: the commit system prompt tells the model to lead with a gitmoji.
        assert!(COMMIT_SYSTEM.contains("gitmoji"));
        assert!(COMMIT_SYSTEM.contains("✨ feat"));
        assert!(COMMIT_SYSTEM.contains("🐛 fix"));
        assert!(COMMIT_SYSTEM.contains("<emoji> <type>"));
    }

    #[test]
    fn split_title_body_unwraps_fenced_markdown() {
        let (t, b) = split_title_body("```markdown\nfeat: x\n\nbody here\n```");
        assert_eq!(t, "feat: x");
        assert_eq!(b, "body here");
    }
}
