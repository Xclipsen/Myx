//! The keyboard-first Spotify Jam overlay.

use super::*;
use crate::*;

const SESSION_PREFERRED_WIDTH: u16 = 64;
const QR_GAP: u16 = 1;
const OVERLAY_HORIZONTAL_PADDING: u16 = 4;
const OVERLAY_VERTICAL_PADDING: u16 = 2;
const MAX_VISIBLE_MEMBERS: usize = 6;

pub(crate) fn render_jam_overlay(f: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let Some(overlay) = app.jam.overlay.as_ref() else {
        return;
    };
    let rect = jam_rect(
        area,
        overlay.screen,
        app.jam.session.as_ref(),
        qr_dimensions(&app.jam.qr),
    );
    f.render_widget(Clear, rect);
    f.render_widget(Block::default().style(theme.element()), rect);
    let inner = rect.inner(Margin::new(2, 1));
    if inner.width == 0 || inner.height == 0 {
        force_area(f, rect);
        return;
    }

    match overlay.screen {
        JamScreen::Join => render_join(f, app, theme, inner),
        JamScreen::Overview => render_overview(f, app, theme, inner),
    }
    force_area(f, rect);
}

fn jam_rect(
    area: Rect,
    screen: JamScreen,
    session: Option<&JamSession>,
    qr_size: (u16, u16),
) -> Rect {
    let active_overview = matches!(screen, JamScreen::Overview) && session.is_some();
    let preferred_width = if active_overview {
        SESSION_PREFERRED_WIDTH.max(qr_size.0.saturating_add(OVERLAY_HORIZONTAL_PADDING))
    } else {
        62
    };
    let preferred_height = match (screen, session) {
        (JamScreen::Join, _) => 13,
        (_, Some(session)) => session_content_height(session)
            .saturating_add(if qr_size.1 > 0 {
                QR_GAP.saturating_add(qr_size.1)
            } else {
                0
            })
            .saturating_add(OVERLAY_VERTICAL_PADDING),
        _ => 13,
    };
    let width = area.width.saturating_sub(2).min(preferred_width);
    let height = area.height.saturating_sub(2).min(preferred_height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn title(theme: Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("Spotify Jam", theme.heading()),
        Span::styled(
            "  EXPERIMENTAL",
            Style::default()
                .fg(theme.warning.into())
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn render_join(f: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let Some(overlay) = app.jam.overlay.as_ref() else {
        return;
    };
    let (before, after) = split_at_cursor(overlay.query(), overlay.join_input.cursor().1);
    let input = format!("{before}▏{after}");
    let mode = overlay.participation.label();
    let lines = vec![
        title(theme),
        Line::raw(""),
        Line::from(Span::styled(
            "Paste a Jam link, URI or token",
            theme.muted(),
        )),
        Line::from(vec![
            Span::styled("› ", theme.heading()),
            Span::styled(
                truncate(&input, area.width.saturating_sub(2) as usize),
                Style::default().fg(theme.text.into()),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Participation  ", theme.muted()),
            Span::styled(mode, theme.heading()),
            Span::styled("  Tab to change", theme.muted()),
        ]),
        Line::raw(""),
        status_line(app, theme),
        Line::from(vec![
            key("Enter", theme),
            Span::styled(" join   ", theme.muted()),
            key("Esc", theme),
            Span::styled(" back", theme.muted()),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn render_overview(f: &mut Frame, app: &App, theme: Theme, area: Rect) {
    let Some(session) = app.jam.session.as_ref() else {
        let state = if app.jam.loading || !app.jam.known {
            "Checking Spotify for an active Jam…"
        } else {
            "No active Jam"
        };
        let lines = vec![
            title(theme),
            Line::raw(""),
            Line::from(Span::styled(state, theme.heading())),
            Line::raw(""),
            Line::from(Span::styled(
                "Start a session here or join with an invite.",
                theme.muted(),
            )),
            Line::raw(""),
            status_line(app, theme),
            Line::from(vec![
                key("s", theme),
                Span::styled(" start   ", theme.muted()),
                key("i", theme),
                Span::styled(" join   ", theme.muted()),
                key("r", theme),
                Span::styled(" refresh   ", theme.muted()),
                key("Esc", theme),
                Span::styled(" close", theme.muted()),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), area);
        return;
    };

    let (qr_width, qr_height) = qr_dimensions(&app.jam.qr);
    let content_height = session_content_height(session);
    let show_qr = qr_width > 0
        && area.width >= qr_width
        && area.height
            >= content_height
                .saturating_add(QR_GAP)
                .saturating_add(qr_height);
    if show_qr {
        let session_area = Rect {
            height: area.height.saturating_sub(qr_height + QR_GAP),
            ..area
        };
        let qr_area = Rect {
            x: area.x + area.width.saturating_sub(qr_width) / 2,
            y: session_area.bottom().saturating_add(QR_GAP),
            width: qr_width,
            height: qr_height,
        };
        render_session(f, app, session, theme, session_area);
        f.render_widget(
            Paragraph::new(
                app.jam
                    .qr
                    .iter()
                    .map(|line| Line::raw(line.clone()))
                    .collect::<Vec<_>>(),
            )
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(ratatui::style::Color::White)
                    .bg(ratatui::style::Color::Black),
            ),
            qr_area,
        );
    } else {
        render_session(f, app, session, theme, area);
    }
}

fn render_session(f: &mut Frame, app: &App, session: &JamSession, theme: Theme, area: Rect) {
    let role = if session.is_owner { "Host" } else { "Guest" };
    let mut lines = vec![
        title(theme),
        Line::from(vec![
            Span::styled(format!("{role}  "), theme.heading()),
            Span::styled(session.session_type.label(), theme.muted()),
            Span::styled(
                format!("  ·  {} participant(s)", session.members.len()),
                theme.muted(),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Participants", theme.heading())),
    ];

    let selected = app
        .jam
        .overlay
        .as_ref()
        .map_or(0, |overlay| overlay.selected_member);
    let fixed_rows = if session.is_owner { 13 } else { 11 };
    let member_rows = area.height.saturating_sub(fixed_rows) as usize;
    let start = selected.saturating_sub(member_rows.saturating_sub(1));
    for (index, member) in session
        .members
        .iter()
        .enumerate()
        .skip(start)
        .take(member_rows.max(1))
    {
        let marker = if index == selected { "› " } else { "  " };
        let owner = if member.id == session.owner_id {
            "  host"
        } else {
            ""
        };
        let control = if member.is_controlling {
            "  control"
        } else {
            ""
        };
        let listening = if member.is_listening {
            "  listening"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(marker, theme.heading()),
            Span::styled(
                truncate(member.label(), area.width.saturating_sub(24) as usize),
                if index == selected {
                    Style::default()
                        .fg(theme.text.into())
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme.muted()
                },
            ),
            Span::styled(owner, theme.muted()),
            Span::styled(control, Style::default().fg(theme.accent.into())),
            Span::styled(listening, Style::default().fg(theme.success.into())),
        ]));
    }
    if session.members.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Waiting for participants…",
            theme.muted(),
        )));
    }

    lines.push(Line::raw(""));
    let invite = session.share_url();
    let invite = invite.as_deref().unwrap_or("invite unavailable");
    lines.push(Line::from(vec![
        Span::styled("Invite  ", theme.heading()),
        Span::styled(
            truncate(invite, area.width.saturating_sub(8) as usize),
            theme.muted(),
        ),
    ]));
    if session.is_owner {
        lines.push(toggle_line("p", "Queue only", session.queue_only, theme));
        lines.push(toggle_line(
            "v",
            "Participant volume",
            session.participant_volume,
            theme,
        ));
    }
    lines.push(status_line(app, theme));
    lines.push(Line::raw(""));
    let mut keys = vec![
        key("c", theme),
        Span::styled(" copy  ", theme.muted()),
        key("o", theme),
        Span::styled(" open  ", theme.muted()),
        key("r", theme),
        Span::styled(" refresh  ", theme.muted()),
    ];
    if session.is_owner {
        keys.extend([
            key("Del", theme),
            Span::styled(" remove  ", theme.muted()),
            key("X", theme),
            Span::styled(" end", theme.muted()),
        ]);
    } else {
        keys.extend([key("X", theme), Span::styled(" leave", theme.muted())]);
    }
    lines.push(Line::from(keys));
    lines.push(Line::from(vec![
        key("↑↓", theme),
        Span::styled(" select   ", theme.muted()),
        key("Esc", theme),
        Span::styled(" close", theme.muted()),
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn qr_dimensions(qr: &[String]) -> (u16, u16) {
    let width = qr
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default()
        .min(u16::MAX as usize) as u16;
    let height = qr.len().min(u16::MAX as usize) as u16;
    (width, height)
}

fn session_content_height(session: &JamSession) -> u16 {
    let fixed_rows = if session.is_owner { 13 } else { 11 };
    let member_rows = session.members.len().clamp(1, MAX_VISIBLE_MEMBERS) as u16;
    fixed_rows + member_rows
}

fn toggle_line(
    key_name: &'static str,
    label: &'static str,
    value: Option<bool>,
    theme: Theme,
) -> Line<'static> {
    let (state, color) = match value {
        Some(true) => ("on", theme.success),
        Some(false) => ("off", theme.text_muted),
        None => ("unknown", theme.text_muted),
    };
    Line::from(vec![
        key(key_name, theme),
        Span::styled(format!(" {label}  "), theme.muted()),
        Span::styled(state, Style::default().fg(color.into())),
    ])
}

fn status_line(app: &App, theme: Theme) -> Line<'static> {
    if app.jam.loading {
        Line::from(vec![
            Span::styled("⠹ ", theme.heading()),
            Span::styled(app.jam.message.clone(), theme.muted()),
        ])
    } else if app.jam.message.is_empty() {
        Line::raw("")
    } else {
        Line::from(Span::styled(app.jam.message.clone(), theme.muted()))
    }
}

fn key(name: &'static str, theme: Theme) -> Span<'static> {
    Span::styled(name, Style::default().fg(theme.primary.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jam_rect_never_exceeds_its_area() {
        for area in [
            Rect::new(0, 0, 1, 1),
            Rect::new(3, 4, 40, 12),
            Rect::new(0, 0, 120, 50),
        ] {
            let rect = jam_rect(area, JamScreen::Overview, None, (0, 0));
            assert!(rect.right() <= area.right());
            assert!(rect.bottom() <= area.bottom());
        }
    }

    #[test]
    fn jam_rect_stacks_qr_below_session_without_exceeding_terminal() {
        let area = Rect::new(0, 0, 120, 50);
        let session = JamSession::default();
        let rect = jam_rect(area, JamScreen::Overview, Some(&session), (49, 25));
        assert_eq!(rect.width, 64);
        assert_eq!(rect.height, 40);
        let inner = rect.inner(Margin::new(2, 1));
        assert!(inner.width >= 49);
        assert_eq!(inner.height, session_content_height(&session) + QR_GAP + 25);
        assert!(rect.right() <= area.right());
        assert!(rect.bottom() <= area.bottom());
    }
}
