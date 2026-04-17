use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use termimad::MadSkin;

pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.dim} {msg:.dim}")
            .unwrap()
            .tick_strings(&["⢆", "⠇", "⠋", "⠙", "⠸", "⠰", "⠠", "⠄", ""]),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(Duration::from_millis(90));
    pb
}

pub fn render_markdown(md: &str) {
    let skin = MadSkin::default_dark();
    skin.print_text(md);
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
