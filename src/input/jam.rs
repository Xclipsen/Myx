//! Keyboard input and asynchronous operations for the Jam overlay.

use crate::*;

pub(crate) fn open_jam(app: &mut App, jam_tx: &flume::Sender<JamReply>) {
    if !app.jam.enabled {
        app.status = "Jam is experimental; set experimental_jam = true".to_string();
        return;
    }
    app.view.actions = None;
    app.view.action_anchor = None;
    app.view.equalizer = None;
    app.jam.overlay = Some(JamOverlay::default());
    spawn_jam_operation(app, JamOperation::Refresh, jam_tx.clone());
}

pub(crate) fn handle_jam_key(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
    jam_tx: &flume::Sender<JamReply>,
) {
    let screen = app
        .jam
        .overlay
        .as_ref()
        .map(|overlay| overlay.screen)
        .unwrap_or(JamScreen::Overview);
    match screen {
        JamScreen::Join => handle_join_key(app, code, mods, jam_tx),
        JamScreen::Overview => handle_overview_key(app, code, jam_tx),
    }
}

fn handle_join_key(
    app: &mut App,
    code: KeyCode,
    mods: KeyModifiers,
    jam_tx: &flume::Sender<JamReply>,
) {
    match code {
        KeyCode::Esc => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay.screen = JamScreen::Overview;
            }
            app.jam.message.clear();
        }
        KeyCode::Tab | KeyCode::BackTab => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay.participation = match overlay.participation {
                    JamType::InPerson => JamType::Remote,
                    _ => JamType::InPerson,
                };
            }
        }
        KeyCode::Enter if !app.jam.loading => {
            let Some(overlay) = app.jam.overlay.as_ref() else {
                return;
            };
            let invite = overlay.query().trim().to_string();
            let participation = overlay.participation;
            if invite.is_empty() {
                app.jam.message = "paste a Jam link, URI or token".to_string();
                return;
            }
            spawn_jam_operation(
                app,
                JamOperation::Join {
                    invite,
                    participation,
                },
                jam_tx.clone(),
            );
        }
        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay.clear_query();
            }
        }
        _ if !app.jam.loading => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay
                    .join_input
                    .input(crossterm::event::KeyEvent::new(code, mods));
            }
        }
        _ => {}
    }
}

fn handle_overview_key(app: &mut App, code: KeyCode, jam_tx: &flume::Sender<JamReply>) {
    if matches!(code, KeyCode::Esc | KeyCode::Char('J')) {
        app.jam.overlay = None;
        app.jam.message.clear();
        return;
    }
    if code == KeyCode::Char('r') && !app.jam.loading {
        spawn_jam_operation(app, JamOperation::Refresh, jam_tx.clone());
        return;
    }

    let Some(session) = app.jam.session.clone() else {
        if app.jam.loading {
            return;
        }
        match code {
            KeyCode::Char('s') => spawn_jam_operation(app, JamOperation::Start, jam_tx.clone()),
            KeyCode::Char('i') => {
                if let Some(overlay) = app.jam.overlay.as_mut() {
                    overlay.screen = JamScreen::Join;
                    overlay.clear_query();
                }
                app.jam.message.clear();
            }
            _ => {}
        }
        return;
    };

    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay.selected_member = overlay.selected_member.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(overlay) = app.jam.overlay.as_mut() {
                overlay.selected_member =
                    (overlay.selected_member + 1).min(session.members.len().saturating_sub(1));
            }
        }
        KeyCode::Char('c') => {
            let invite = session.share_url();
            app.jam.message = match invite.as_deref() {
                Some(invite) if copy_to_clipboard(invite) => "invite copied".to_string(),
                Some(_) => "clipboard unavailable".to_string(),
                None => "Spotify returned no invite link".to_string(),
            };
        }
        KeyCode::Char('o') => {
            app.jam.message = match session.share_url().map(open::that) {
                Some(Ok(())) => "invite opened".to_string(),
                Some(Err(_)) => "could not open invite".to_string(),
                None => "Spotify returned no invite link".to_string(),
            };
        }
        KeyCode::Char('p') if session.is_owner && !app.jam.loading => {
            spawn_jam_operation(
                app,
                JamOperation::QueueOnly(!session.queue_only.unwrap_or(false)),
                jam_tx.clone(),
            );
        }
        KeyCode::Char('v') if session.is_owner && !app.jam.loading => {
            spawn_jam_operation(
                app,
                JamOperation::ParticipantVolume(!session.participant_volume.unwrap_or(false)),
                jam_tx.clone(),
            );
        }
        KeyCode::Delete | KeyCode::Backspace if session.is_owner && !app.jam.loading => {
            let selected = app
                .jam
                .overlay
                .as_ref()
                .map_or(0, |overlay| overlay.selected_member);
            let Some(member) = session.members.get(selected) else {
                return;
            };
            if member.id == session.owner_id {
                app.jam.message = "the host cannot be removed".to_string();
                return;
            }
            spawn_jam_operation(
                app,
                JamOperation::Kick {
                    session_id: session.session_id,
                    member_id: member.id.clone(),
                },
                jam_tx.clone(),
            );
        }
        KeyCode::Char('X') if !app.jam.loading => {
            let operation = if session.is_owner {
                JamOperation::End {
                    session_id: session.session_id,
                }
            } else {
                JamOperation::Leave {
                    session_id: session.session_id,
                }
            };
            spawn_jam_operation(app, operation, jam_tx.clone());
        }
        _ => {}
    }
}

pub(crate) fn spawn_jam_operation(
    app: &mut App,
    operation: JamOperation,
    tx: flume::Sender<JamReply>,
) {
    if !app.jam.enabled || app.jam.loading {
        return;
    }
    app.jam.loading = true;
    if !matches!(operation, JamOperation::Refresh) || app.jam.overlay.is_some() {
        app.jam.message = operation.pending_label().to_string();
    }
    let engine = app.svc.engine.clone();
    let task = operation.clone();
    tokio::spawn(async move {
        let result = run_operation(&engine, &task)
            .await
            .map_err(|error| format!("{error:#}"));
        let _ = tx.send(JamReply { operation, result });
    });
}

async fn run_operation(
    engine: &Engine,
    operation: &JamOperation,
) -> anyhow::Result<Option<JamSession>> {
    match operation {
        JamOperation::Refresh => engine.jam_current().await,
        JamOperation::Start => engine.jam_start().await.map(Some),
        JamOperation::Join {
            invite,
            participation,
        } => engine.jam_join(invite, *participation).await.map(Some),
        JamOperation::End { session_id } => {
            engine.jam_end(session_id).await?;
            Ok(None)
        }
        JamOperation::Leave { session_id } => {
            engine.jam_leave(session_id).await?;
            Ok(None)
        }
        JamOperation::Kick {
            session_id,
            member_id,
        } => {
            engine.jam_kick(session_id, member_id).await?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            engine.jam_current().await
        }
        JamOperation::QueueOnly(enabled) => {
            engine.jam_set_queue_only(*enabled).await?;
            let mut session = engine.jam_current().await?;
            if let Some(session) = session.as_mut() {
                session.queue_only = Some(*enabled);
            }
            Ok(session)
        }
        JamOperation::ParticipantVolume(enabled) => {
            engine.jam_set_participant_volume(*enabled).await?;
            let mut session = engine.jam_current().await?;
            if let Some(session) = session.as_mut() {
                session.participant_volume = Some(*enabled);
            }
            Ok(session)
        }
    }
}

pub(crate) fn apply_jam_reply(app: &mut App, reply: JamReply) {
    app.jam.loading = false;
    match reply.result {
        Ok(session) => {
            let joined = matches!(reply.operation, JamOperation::Join { .. });
            app.jam.set_session(session);
            if joined {
                if let Some(overlay) = app.jam.overlay.as_mut() {
                    overlay.screen = JamScreen::Overview;
                }
            }
            app.jam.message = reply.operation.success_label().to_string();
            if !app.jam.message.is_empty() {
                app.status = app.jam.message.clone();
            }
        }
        Err(error) => {
            app.jam.known = true;
            app.jam.message = error;
            app.status = "Jam action failed".to_string();
        }
    }
}
