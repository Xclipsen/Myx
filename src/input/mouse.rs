//! Mouse input: clicks, drags, and wheel scrolls, resolved against the hit
//! rects the renderer left in `FrameOut`.

use crate::*;

/// Mouse input. Mirrors `handle_key`: returns `true` when the app should quit.
pub(crate) fn handle_mouse(
    app: &mut App,
    out: &FrameOut,
    m: crossterm::event::MouseEvent,
    chans: &UiChannels,
) -> bool {
    // Jam is modal and intentionally keyboard-first. Do not let a click through
    // to playback, the queue or a library row underneath it.
    if app.jam.overlay.is_some() {
        return false;
    }
    // The equalizer is modal: its own hit rects get first refusal and every
    // other mouse event is swallowed so nothing hidden behind it moves.
    if app.view.equalizer.is_some() {
        handle_equalizer_mouse(app, out, m);
        return false;
    }

    if m.kind == MouseEventKind::Down(MouseButton::Right) {
        app.view.actions = None;
        app.view.action_anchor = None;
        if let Some(idx) = out
            .hits
            .lib
            .and_then(|rect| library_row_at(rect, out.lib_offset, m.column, m.row))
        {
            if let Some(item) = app
                .cur_items()
                .get(idx)
                .filter(|item| item.is_track)
                .cloned()
            {
                app.browse.selected = idx;
                app.session.last_click = None;
                app.view.actions = Some(ActionMenu {
                    title: item.name,
                    items: vec![ActionItem {
                        label: "+ Add to Queue".to_string(),
                        kind: ActionKind::Queue { uri: item.uri },
                    }],
                    selected: 0,
                });
                app.view.action_anchor = Some((m.column.saturating_add(1), m.row));
            }
        }
        return false;
    }

    // An open menu owns mouse input. Clicking an entry activates it; clicking
    // elsewhere dismisses it without also activating the underlying UI.
    if app.view.actions.is_some() {
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            let selected = out.hits.actions.iter().position(|rect| {
                m.column >= rect.x
                    && m.column < rect.right()
                    && m.row >= rect.y
                    && m.row < rect.bottom()
            });
            if let Some(selected) = selected {
                if let Some(menu) = app.view.actions.as_mut() {
                    menu.selected = selected;
                }
                handle_action_key(app, KeyCode::Enter, &chans.detail, &chans.astatus);
            } else {
                app.view.actions = None;
                app.view.action_anchor = None;
            }
        }
        return false;
    }

    if matches!(
        m.kind,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
    ) {
        let is_down = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        let mut consumed = false;
        // Drag the sidebar scrollbar (2-col grab target) to scroll.
        if let Some(sb) = out.hits.scroll {
            if m.column + 1 >= sb.x
                && m.column <= sb.x
                && m.row >= sb.y
                && m.row < sb.y + sb.height
                && sb.height > 0
            {
                consumed = true;
                let total = out.hits.scroll_len;
                if total > 1 {
                    let denom = sb.height.saturating_sub(1).max(1) as f32;
                    let frac = (m.row - sb.y) as f32 / denom;
                    let sel = (frac * (total - 1) as f32).round() as usize;
                    app.browse.selected = sel.min(total - 1);
                    app.normalize_selection();
                }
            }
        }
        // Click/drag the volume meter bars to set volume.
        if !consumed {
            if let Some(vr) = out.hits.vol {
                if m.row == vr.y && m.column >= vr.x && m.column < vr.x + vr.width && vr.width > 0 {
                    consumed = true;
                    let offset = (m.column - vr.x) as u32;
                    let vol = (((offset + 1) * 100) / vr.width as u32).min(100) as u8;
                    app.transport.volume = vol;
                    let _ = app.svc.engine.set_volume(vol_u16(app.transport.volume));
                }
            }
        }
        // Otherwise an initial click on the progress bar seeks.
        if !consumed && is_down {
            if let Some(bar) = out.hits.bar {
                if m.row == bar.y
                    && m.column >= bar.x
                    && m.column < bar.x + bar.width
                    && bar.width > 0
                {
                    if let Some(dur) = app.playback.now.as_ref().map(|n| n.duration_ms) {
                        let frac = (m.column - bar.x) as f32 / bar.width as f32;
                        app.playback
                            .seek_to(&app.svc.engine, (frac * dur as f32) as u32);
                    }
                }
            }
        }
        // A queue row starts that track and keeps everything after it as the
        // new playback context. The displayed queue is already ordered, so do
        // not reshuffle it even when the global shuffle toggle is active.
        if !consumed && is_down {
            let queue_index = queue_row_at(&out.hits.queue, m.column, m.row);
            if let Some(index) = queue_index {
                // Clicking a row is also how the queue takes the keyboard, so
                // the cursor picks up where the pointer left off.
                app.view.focus = Focus::Queue;
                app.transport.queue_selected = index;
                play_queue_item(app, index);
                app.normalize_queue_selection();
                consumed = true;
            }
        }
        // View-tab click -> switch the right pane.
        if !consumed && is_down {
            let hit = out
                .hits
                .tabs
                .iter()
                .find(|(_, r)| m.row == r.y && m.column >= r.x && m.column < r.x + r.width)
                .map(|(v, _)| *v);
            if let Some(v) = hit {
                app.view.mode = v;
                // Picking a view by hand overrides any view `Tab` displaced.
                app.view.queue_return = None;
                // Follow the pointer: the Queue tab takes the keyboard, any
                // other tab hands it back. Deciding this from the view just
                // clicked rather than from `queue_pane` matters — the hit
                // rects still describe the frame drawn *before* this click.
                app.view.focus = if v == RightView::Queue {
                    Focus::Queue
                } else {
                    Focus::Library
                };
                if v == RightView::Queue
                    && (app.session.reclaimed || app.transport.playback_started)
                {
                    spawn_queue_fetch(app.svc.webapi.clone(), chans.queue.clone());
                }
                consumed = true;
            }
        }
        // Library click -> select; double-click (same row <400ms) -> activate.
        if !consumed && is_down {
            if let Some(lr) = out.hits.lib {
                if m.column >= lr.x
                    && m.column < lr.x + lr.width
                    && m.row >= lr.y
                    && m.row < lr.y + lr.height
                {
                    let idx = out.lib_offset + (m.row - lr.y) as usize;
                    let selectable = app
                        .cur_items()
                        .get(idx)
                        .map(|it| !it.is_header)
                        .unwrap_or(false);
                    if selectable {
                        app.browse.selected = idx;
                        app.view.focus = Focus::Library;
                        let now = Instant::now();
                        let dbl = app
                            .session
                            .last_click
                            .map(|(r0, t0)| {
                                r0 == m.row && now.duration_since(t0) < Duration::from_millis(400)
                            })
                            .unwrap_or(false);
                        if dbl {
                            app.session.last_click = None;
                            let quit =
                                handle_key(app, out, KeyCode::Enter, KeyModifiers::empty(), chans);
                            if quit {
                                return true;
                            }
                        } else {
                            app.session.last_click = Some((m.row, now));
                        }
                    }
                }
            }
        }
    }
    // Scroll the list under the pointer; only the volume meter owns wheel
    // volume changes, so browsing cannot alter playback volume by accident.
    if matches!(
        m.kind,
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    ) {
        let over = |rect: Rect| {
            m.column >= rect.x
                && m.column < rect.x + rect.width
                && m.row >= rect.y
                && m.row < rect.y + rect.height
        };
        if out.hits.lib.is_some_and(over) {
            app.move_sel(if m.kind == MouseEventKind::ScrollUp {
                -1
            } else {
                1
            });
        } else if out.hits.vol.is_some_and(over) {
            match m.kind {
                MouseEventKind::ScrollUp => {
                    app.transport.volume = (app.transport.volume + 5).min(100);
                }
                MouseEventKind::ScrollDown => {
                    app.transport.volume = app.transport.volume.saturating_sub(5);
                }
                _ => {}
            }
            let _ = app.svc.engine.set_volume(vol_u16(app.transport.volume));
        }
    }
    false
}

pub(crate) fn library_row_at(rect: Rect, offset: usize, column: u16, row: u16) -> Option<usize> {
    point_in_rect(rect, column, row).then(|| offset + (row - rect.y) as usize)
}

pub(crate) fn queue_row_at(rows: &[(usize, Rect)], column: u16, row: u16) -> Option<usize> {
    rows.iter()
        .find(|(_, rect)| point_in_rect(*rect, column, row))
        .map(|(index, _)| *index)
}

fn point_in_rect(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}

pub(crate) fn play_queue_item(app: &mut App, index: usize) {
    let Some(uri) = app.transport.queue_uris.get(index).cloned() else {
        return;
    };
    let title = app
        .transport
        .queue
        .get(index)
        .cloned()
        .unwrap_or_else(|| "queue track".to_string());
    let tracks = app.transport.queue_uris[index..].to_vec();

    app.status = format!("starting {title}…");
    match app.svc.engine.play_tracks(tracks, Some(uri), 0, false) {
        Ok(()) => {
            let displayed_through = (index + 1).min(app.transport.queue.len());
            app.transport.queue.drain(..displayed_through);
            app.transport.queue_uris.drain(..=index);
            app.transport.source = PlaySource::None;
            app.transport.source_name = "Queue".to_string();
            app.transport.playback_started = true;
            app.session.reclaimed = false;
        }
        Err(error) => app.status = format!("couldn't play: {error:#}"),
    }
}
