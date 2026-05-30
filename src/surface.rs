use crate::hud::{fit_line, HudSnapshot};

pub fn render_compact(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    let mut lines = vec![
        fit_line(snapshot.compact_headline(), width),
        fit_line(snapshot.compact_context(), width),
    ];

    lines.retain(|line| !line.is_empty());
    if lines.len() > 2 {
        lines.truncate(2);
    }
    lines
}

pub fn render_expanded(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    let mut lines = render_compact(snapshot, width);
    for detail in snapshot.expanded_details() {
        lines.push(fit_line(detail, width));
    }
    lines
}
