//! Terminal key input — the main keymap.

use crate::*;

/// Returns true if the app should quit.
pub(crate) fn handle_key(
    app: &mut App,
    out: &FrameOut,
    code: KeyCode,
    mods: KeyModifiers,
    chans: &UiChannels,
) -> bool {
    // --- Actions menu captures input while open ---
    // Double-press Ctrl-C to quit (works from anywhere). Single press arms it.
    if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
        let now = Instant::now();
        if app
            .session
            .last_ctrl_c
            .map(|t| now.duration_since(t) < Duration::from_millis(1500))
            .unwrap_or(false)
        {
            return true;
        }
        app.session.last_ctrl_c = Some(now);
        app.status = "press Ctrl-C again to quit".to_string();
        return false;
    }

    if app.view.actions.is_some() {
        handle_action_key(app, code, &chans.detail, &chans.astatus);
        return false;
    }

    if app.jam.overlay.is_some() {
        handle_jam_key(app, code, mods, &chans.jam);
        return false;
    }

    if app.view.equalizer.is_some() {
        handle_equalizer_key(app, code);
        return false;
    }

    // --- Search input mode captures everything ---
    if app.search.input_mode {
        match code {
            KeyCode::Esc => app.search.input_mode = false,
            KeyCode::Enter => {
                app.search.input_mode = false;
                let q = app.search.query().trim().to_string();
                if !q.is_empty() {
                    if app.in_playlist() {
                        app.filter_playlist();
                        app.status = if app.cur_items().is_empty() {
                            "no matching tracks".to_string()
                        } else {
                            String::new()
                        };
                    } else {
                        app.search.searching = true;
                        app.search.in_flight = true;
                        app.browse.selected = 0;
                        app.status = "searching…".to_string();
                        spawn_search(app.svc.webapi.clone(), q, chans.search.clone());
                    }
                }
            }
            // Ctrl-U clears the query — readline muscle memory. The fork
            // binds Ctrl-U to undo; shadow it (same call as agent-runtime).
            KeyCode::Char('u') if mods.contains(crossterm::event::KeyModifiers::CONTROL) => {
                app.search.clear();
            }
            // Everything else — typing, cursor movement, word ops — is the
            // editor's business. Enter is intercepted above, so no newlines.
            _ => {
                app.search
                    .input
                    .input(crossterm::event::KeyEvent::new(code, mods));
            }
        }
        return false;
    }

    // Zen hides the library, so the keys that drive one do nothing rather than
    // moving a selection nobody can see. Placed after the overlays above, which
    // stay usable if one was already open when zen came on. The movement keys
    // are exempt while the queue holds focus: that pane is still on screen in
    // zen's split, so they are driving something visible.
    if app.view.zen && !queue_owns_movement(app, code) && drives_library(code, mods) {
        return false;
    }

    match code {
        KeyCode::Char('J') => open_jam(app, &chans.jam),
        KeyCode::Char('e') => {
            app.view.equalizer = Some(EqualizerOverlay::default());
        }
        KeyCode::Char('/') => {
            app.search.input_mode = true;
            app.search.clear();
        }
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            if app.view.focus == Focus::Queue {
                // Esc backs out of the queue before it backs out of anything
                // in the library underneath it.
                unfocus_queue(app);
            } else if app.search.playlist_results.take().is_some() {
                app.search.clear();
                app.browse.selected = app.first_selectable();
            } else if let Some(d) = app.browse.details.pop() {
                app.browse.selected = d.parent_selected;
            } else if app.search.searching {
                app.search.searching = false;
                app.browse.selected = 0;
            }
            // Nothing to back out of — Esc no longer quits (use q or Ctrl-C twice).
        }
        KeyCode::Char(' ') | KeyCode::Char('p') | KeyCode::Media(MediaKeyCode::PlayPause) => {
            if app.transport.playback_started {
                let playing = app.playback.now.as_ref().is_some_and(|now| now.is_playing);
                let result = if playing {
                    app.svc.engine.pause()
                } else {
                    app.svc.engine.play()
                };
                if result.is_ok() {
                    app.playback.set_playing_locally(!playing);
                }
            } else if app.session.reclaimed {
                // Resume the reclaimed server-side context (full queue intact).
                let _ = app.svc.engine.play();
                app.transport.playback_started = true;
            } else {
                // Resume the persisted source (context/radio/liked).
                app.transport.playback_started = resume_source(app, &chans.radio);
            }
        }
        KeyCode::Media(MediaKeyCode::Stop) => {
            app.svc.engine.stop();
        }
        KeyCode::Char('n') | KeyCode::Media(MediaKeyCode::TrackNext) => {
            let _ = app.svc.engine.next();
        }
        KeyCode::Char('b') | KeyCode::Media(MediaKeyCode::TrackPrevious) => {
            let _ = app.svc.engine.prev();
        }
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Media(MediaKeyCode::RaiseVolume) => {
            app.transport.volume = (app.transport.volume + 5).min(100);
            let _ = app.svc.engine.set_volume(vol_u16(app.transport.volume));
        }
        KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Media(MediaKeyCode::LowerVolume) => {
            app.transport.volume = app.transport.volume.saturating_sub(5);
            let _ = app.svc.engine.set_volume(vol_u16(app.transport.volume));
        }
        KeyCode::Char('s') => {
            app.transport.shuffle = !app.transport.shuffle;
            let _ = app.svc.engine.shuffle(app.transport.shuffle);
        }
        // Play the highlighted playlist / album / artist outright. Enter still
        // opens; this is the direct route that used to require two Enters or
        // the actions menu.
        KeyCode::Char('P') => play_selected_context(app, false),
        KeyCode::Char('S') => {
            // Flip the global toggle too, or the footer would show shuffle off
            // while playback is shuffled, and `resume_source` would later
            // replay this context unshuffled.
            app.transport.shuffle = true;
            let _ = app.svc.engine.shuffle(true);
            play_selected_context(app, true);
        }
        KeyCode::Char('R') => {
            app.transport.repeat = !app.transport.repeat;
            let _ = app.svc.engine.repeat(app.transport.repeat);
        }
        KeyCode::Char('r') => {
            app.status = "loading library…".to_string();
            app.browse.library.reset_loading();
            spawn_library_fetch(
                app.svc.webapi.clone(),
                chans.lib.clone(),
                chans.libdone.clone(),
            );
        }
        KeyCode::Char('o') => {
            app.browse.sort = app.browse.sort.next();
            let m = app.browse.sort;
            sort_list(app.cur_list_mut(), m);
            app.browse.selected = app.first_selectable();
            app.status = format!("sorted by {}", m.label());
        }
        KeyCode::Char('a') => {
            // Zen hides the library, so the menu belongs to what is playing —
            // acting on a selection nobody can see is how it ends up offering
            // "remove from Liked" for the wrong track.
            let item = if app.view.zen {
                app.playback
                    .now
                    .as_ref()
                    .filter(|n| !n.uri.is_empty())
                    .map(|n| LibItem::track(n.title.clone(), n.artist.clone(), n.uri.clone()))
            } else {
                app.cur_items().get(app.browse.selected).cloned()
            };
            if let Some(item) = item {
                if !item.is_header && !item.is_play {
                    // Instant menu (no network), then enrich when the API returns.
                    app.view.action_anchor = None;
                    app.view.actions = Some(build_action_menu(None, &item));
                    spawn_action_menu(app.svc.webapi.clone(), item, chans.menu.clone());
                }
            }
        }
        // Tab / Shift+Tab move the keyboard between the two navigable lists.
        // Both directions do the same thing — there are only two panes.
        KeyCode::Tab | KeyCode::BackTab => toggle_focus(app, out, chans),
        // Shift+arrows seek ±5s; plain arrows (and [ ]) rotate the sections.
        KeyCode::Right if mods.contains(KeyModifiers::SHIFT) => {
            app.playback.seek_step(SEEK_STEP_MS)
        }
        KeyCode::Left if mods.contains(KeyModifiers::SHIFT) => {
            app.playback.seek_step(-SEEK_STEP_MS)
        }
        KeyCode::Right | KeyCode::Char(']') => shift_section(app, 1),
        KeyCode::Left | KeyCode::Char('[') => shift_section(app, -1),
        // Lyrics is a toggle off Now Playing rather than a stop on a rotation:
        // the arrows that used to rotate the right pane now drive the sections.
        KeyCode::Char('l') => {
            app.view.mode = if app.view.mode == RightView::Lyrics {
                RightView::NowPlaying
            } else {
                RightView::Lyrics
            };
            // Neither view shows a queue pane, so the keyboard comes back —
            // to the view just chosen, not to whatever `Tab` once displaced.
            app.view.queue_return = None;
            app.view.focus = Focus::Library;
        }
        // The frame loop notices the layout change and wipes the art box.
        KeyCode::Char('z') => app.view.zen = !app.view.zen,
        KeyCode::Down | KeyCode::Char('j') => move_focused(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_focused(app, -1),
        // Needs a terminal that reports modified Enter (kitty, WezTerm, foot).
        KeyCode::Enter if mods.contains(KeyModifiers::SHIFT) => play_selected_context(app, false),
        // A queue row plays from there; the library's Enter opens or plays.
        KeyCode::Enter if app.view.focus == Focus::Queue => {
            play_queue_item(app, app.transport.queue_selected)
        }
        KeyCode::Enter => match app.activate() {
            Activated::Open(uri, name) => {
                app.search.playlist_results = None;
                app.status = format!("loading {name}…");
                spawn_detail_fetch(
                    app.svc.webapi.clone(),
                    app.svc.engine.session(),
                    uri,
                    name,
                    chans.detail.clone(),
                );
            }
            Activated::Radio(uri) => {
                app.status = "starting radio…".to_string();
                let session = app.svc.engine.session();
                let tx = chans.radio.clone();
                tokio::spawn(async move {
                    let res = match tokio::time::timeout(
                        Duration::from_secs(12),
                        engine::radio_tracks(&session, &uri),
                    )
                    .await
                    {
                        Ok(r) => r.map_err(|e| e.to_string()),
                        Err(_) => {
                            Err("timed out (mercury radio endpoint unresponsive)".to_string())
                        }
                    };
                    let _ = tx.send(res.map(|uris| Radio {
                        uris,
                        start_position_ms: 0,
                    }));
                });
            }
            Activated::None => {}
        },
        _ => {}
    }
    false
}

/// Rotate the library sections, which also brings the keyboard back to them —
/// stepping through sections while the cursor sits in the queue would move a
/// selection off screen.
fn shift_section(app: &mut App, delta: isize) {
    app.search.searching = false;
    app.browse.section = app.browse.section.shift(delta);
    app.browse.selected = app.first_selectable();
    app.view.focus = Focus::Library;
}

/// Move the cursor of whichever list currently holds focus.
fn move_focused(app: &mut App, dir: isize) {
    match app.view.focus {
        Focus::Library => app.move_sel(dir),
        Focus::Queue => app.move_queue_sel(dir),
    }
}

/// `Tab`: hand the keyboard to the queue pane, or take it back.
///
/// Focusing the queue has to guarantee it is on screen. The wide Now Playing
/// layout already renders it beside the cover; every narrower layout has to
/// switch the right pane over. `queue_pane` is the renderer's own answer to
/// "was one drawn", which keeps that decision from re-deriving the layout
/// thresholds here and drifting out of step with them.
fn toggle_focus(app: &mut App, out: &FrameOut, chans: &UiChannels) {
    if app.view.focus == Focus::Queue {
        unfocus_queue(app);
        return;
    }
    app.view.focus = Focus::Queue;
    if out.hits.queue_pane.is_none() {
        // Nothing drew a queue, so give it the right pane — and remember what
        // that displaced, or leaving the queue would strand the user on it.
        app.view.queue_return = Some(app.view.mode);
        app.view.mode = RightView::Queue;
    }
    // Same gate as every other route to the Queue view: a reclaimed session
    // has a server-side queue worth fetching even before playback started here.
    if app.transport.playback_started || app.session.reclaimed {
        spawn_queue_fetch(app.svc.webapi.clone(), chans.queue.clone());
    }
    app.normalize_queue_selection();
}

/// Hand the keyboard back to the library, restoring whatever view `Tab`
/// displaced to put the queue on screen in the first place.
pub(crate) fn unfocus_queue(app: &mut App) {
    app.view.focus = Focus::Library;
    if let Some(previous) = app.view.queue_return.take() {
        app.view.mode = previous;
    }
}

/// Whether the movement keys belong to the queue rather than the library.
///
/// Only these keys retarget: the library's own commands (`/`, `o`, `r`, `P`,
/// `S`) keep acting on the library even while the queue holds the cursor.
fn queue_owns_movement(app: &App, code: KeyCode) -> bool {
    app.view.focus == Focus::Queue
        && matches!(
            code,
            KeyCode::Up | KeyCode::Down | KeyCode::Enter | KeyCode::Char('j' | 'k')
        )
}

/// Keys whose whole effect is on the library pane.
///
/// Zen hides that pane, so these do nothing there rather than moving a selection
/// nobody can see — and `a` is deliberately absent, because it retargets onto
/// the playing track instead of going quiet. `Tab` is absent too: it moves the
/// cursor to the queue, which zen still shows.
pub(crate) fn drives_library(code: KeyCode, mods: KeyModifiers) -> bool {
    // Shift+arrows seek the playhead; only the bare arrows rotate sections.
    if matches!(code, KeyCode::Left | KeyCode::Right) && mods.contains(KeyModifiers::SHIFT) {
        return false;
    }
    matches!(
        code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::Char('/' | '[' | ']' | 'j' | 'k' | 'o' | 'r' | 'P' | 'S')
    )
}
