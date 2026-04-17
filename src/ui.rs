use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use termimad::crossterm::style::Color::*;
use termimad::{rgb, Alignment, MadSkin, StyledChar};

pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.magenta} {msg:.dim}")
            .unwrap()
            // Smooth braille wave
            .tick_strings(&[
                "⡀", "⡄", "⡆", "⡇", "⣇", "⣧", "⣷", "⣿", "⢿", "⠿", "⠟", "⠏", "⠇", "⠃", "⠁", "",
            ]),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

pub fn render_markdown(md: &str) {
    let mut skin = MadSkin::default_dark();

    // Headings — Dracula-ish gradient
    skin.headers[0].set_fg(rgb(255, 121, 198)); // H1 hot pink
    skin.headers[0].align = Alignment::Left;
    skin.headers[1].set_fg(rgb(139, 233, 253)); // H2 cyan
    skin.headers[2].set_fg(rgb(80, 250, 123)); // H3 green
    skin.headers[3].set_fg(rgb(255, 184, 108)); // H4 orange

    // Emphasis
    skin.bold.set_fg(rgb(255, 184, 108));
    skin.italic.set_fg(rgb(189, 147, 249));
    skin.strikeout.set_fg(DarkGrey);

    // Bullets / quotes / rules
    skin.bullet = StyledChar::from_fg_char(rgb(255, 121, 198), '▸');
    skin.quote_mark = StyledChar::from_fg_char(rgb(139, 233, 253), '┃');
    skin.horizontal_rule = StyledChar::from_fg_char(rgb(98, 114, 164), '─');

    // Inline code + code blocks
    skin.inline_code.set_fg(rgb(241, 250, 140));
    skin.inline_code.set_bg(rgb(40, 42, 54));
    skin.code_block.set_fg(rgb(248, 248, 242));
    skin.code_block.set_bg(rgb(40, 42, 54));

    // Tables
    skin.table.set_fg(rgb(98, 114, 164));

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
