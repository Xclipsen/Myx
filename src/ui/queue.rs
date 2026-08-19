//! The Queue view.

use crate::*;

pub(crate) fn render_queue_view(
    f: &mut Frame,
    app: &App,
    out: &mut FrameOut,
    theme: Theme,
    area: Rect,
) {
    f.render_widget(Block::default().style(theme.panel()), area);
    // Recorded whether or not the queue has rows in it: `Tab` reads this to
    // learn the queue is already on screen, which is true of an empty one too.
    out.hits.queue_pane = Some(area);
    let inner = area.inner(Margin::new(2, 1));
    if inner.height == 0 {
        return;
    }
    let focused = app.view.focus == Focus::Queue;
    let max = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Context header — what's playing from.
    if !app.transport.source_name.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("PLAYING FROM  ", theme.muted()),
            Span::styled(
                truncate(&app.transport.source_name, max.saturating_sub(14)),
                Style::default()
                    .fg(theme.primary.into())
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::raw(""));
    }

    lines.push(Line::from(Span::styled("UP NEXT", theme.heading())));
    lines.push(Line::raw(""));

    let used = lines.len();
    if app.transport.queue.is_empty() {
        lines.push(Line::from(Span::styled("queue is empty", theme.muted())));
    } else {
        for (i, q) in app
            .transport
            .queue
            .iter()
            .take(inner.height.saturating_sub(used as u16) as usize)
            .enumerate()
        {
            out.hits.queue.push((
                i,
                Rect::new(inner.x, inner.y + lines.len() as u16, inner.width, 1),
            ));
            // Mirrors the library's cursor: bold text on the element
            // background, and only while this pane holds the keyboard.
            let selected = focused && i == app.transport.queue_selected;
            let style = if selected {
                Style::default()
                    .fg(theme.text.into())
                    .bg(theme.background_element.into())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.into())
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{:>2}  ", i + 1), theme.muted()),
                Span::styled(truncate(q, max.saturating_sub(4)), style),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}
