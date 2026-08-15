//! The rest of `App`'s parts: services, theme, transport, browse, search, view, session.

use crate::*;

/// Long-lived services the UI talks to. All three are used through `&self`
/// (the `Arc<Mutex<_>>` is only ever cloned), so grouping them costs no
/// borrow flexibility.
pub(crate) struct Services {
    pub(crate) engine: Engine,
    pub(crate) picker: Picker,
    pub(crate) webapi: Arc<Mutex<WebApi>>,
}

pub(crate) const FADE_MS: u64 = 1500;

/// The palette currently on screen, plus the cross-fade walking it towards
/// the incoming track's palette. `displayed` is what every widget reads;
/// `target` is only used to snap exactly on completion.
pub(crate) struct ThemeState {
    pub(crate) displayed: Theme,
    pub(crate) target: Theme,
    pub(crate) fade: Option<ThemeFade>,
}

impl ThemeState {
    pub(crate) fn start_fade(&mut self, to: Theme) {
        self.fade = Some(ThemeFade::new(
            self.displayed,
            to,
            Duration::from_millis(FADE_MS),
        ));
        self.target = to;
    }

    pub(crate) fn advance(&mut self) {
        if let Some(fade) = &self.fade {
            self.displayed = fade.current();
            if fade.is_done() {
                self.displayed = self.target;
                self.fade = None;
            }
        }
    }
}

/// Playback controls and the queue — everything the transport bar and the
/// persisted `SavedState` care about. None of it touches the playhead.
pub(crate) struct Transport {
    pub(crate) shuffle: bool,
    pub(crate) repeat: bool,
    pub(crate) volume: u8, // 0..=100 (mirrors the 50% mixer default)
    pub(crate) queue: Vec<String>,
    pub(crate) queue_uris: Vec<String>,
    // Whether real playback has started this session (gates resume-on-play).
    pub(crate) playback_started: bool,
    // What's playing (context/radio/liked), for faithful resume on reboot.
    pub(crate) source: PlaySource,
    pub(crate) source_name: String,
    /// Local DSP state. Kept with the transport controls because it affects the
    /// audio path and is persisted alongside volume/shuffle/repeat.
    pub(crate) equalizer: EqualizerSettings,
}

/// The library browser: what's loaded, where the cursor is, and the drill-in
/// stack. The viewport offset is not here — it lives in `FrameOut`, since the
/// renderer owns it across frames.
pub(crate) struct BrowseState {
    pub(crate) library: Library,
    pub(crate) section: Section,
    pub(crate) selected: usize,
    pub(crate) sort: SortMode,
    // Drill-in stack (artist → album → …). Topmost is what's shown.
    pub(crate) details: Vec<Detail>,
}

/// The `/` search overlay: whether the prompt is capturing keys, the typed
/// query, and the results that temporarily replace the library list.
pub(crate) struct SearchState {
    pub(crate) input_mode: bool,
    // The prompt's editor. Read through `query()`, reset through `clear()`.
    pub(crate) input: tui_textarea::TextArea<'static>,
    pub(crate) searching: bool,
    // A submitted query whose results have not landed yet. `searching` means
    // "the search view is active"; this means "the wire is hot" — the empty
    // list renders "searching…" instead of "(empty)" while it's set.
    pub(crate) in_flight: bool,
    pub(crate) search_results: Vec<LibItem>,
    pub(crate) playlist_results: Option<Vec<LibItem>>,
}

impl SearchState {
    /// The typed query. First line only — the prompt is single-line (Enter is
    /// intercepted), so this also defuses any newline a paste might smuggle in.
    pub(crate) fn query(&self) -> &str {
        self.input.lines().first().map_or("", String::as_str)
    }

    /// Empty the editor (fresh buffer, cursor at column 0).
    pub(crate) fn clear(&mut self) {
        self.input = tui_textarea::TextArea::default();
    }
}

/// What the user is looking at: the right pane's mode, the zen (sidebar
/// hidden) toggle, the lyrics backing the Lyrics view, and the actions
/// overlay drawn on top of everything.
pub(crate) struct ViewState {
    // Which view fills the right pane.
    pub(crate) mode: RightView,
    // Sidebar hidden, so the right view (and its cover) gets the whole width.
    pub(crate) zen: bool,
    // Lyrics: (timestamp_ms, line). Synced when timestamps are non-zero.
    pub(crate) lyrics: Vec<(u32, String)>,
    pub(crate) lyrics_synced: bool,
    // Context actions menu overlay (opened with `a`).
    pub(crate) actions: Option<ActionMenu>,
    // Mouse position for a compact context popup; `None` keeps the centered
    // keyboard-driven actions overlay.
    pub(crate) action_anchor: Option<(u16, u16)>,
    // Ten-band equalizer overlay (opened with `e`).
    pub(crate) equalizer: Option<EqualizerOverlay>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EqualizerOverlay {
    pub(crate) selected_band: usize,
}

/// Runtime-only Jam state. Nothing here is persisted: Spotify remains the
/// authority for membership, permissions and the invitation token.
pub(crate) struct JamState {
    pub(crate) enabled: bool,
    pub(crate) known: bool,
    pub(crate) loading: bool,
    pub(crate) session: Option<JamSession>,
    pub(crate) overlay: Option<JamOverlay>,
    pub(crate) message: String,
    pub(crate) qr: Vec<String>,
}

impl JamState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            known: false,
            loading: false,
            session: None,
            overlay: None,
            message: String::new(),
            qr: Vec::new(),
        }
    }

    pub(crate) fn set_session(&mut self, mut session: Option<JamSession>) {
        if let (Some(next), Some(previous)) = (session.as_mut(), self.session.as_ref()) {
            next.merge_known_controls_from(previous);
        }
        let previous_invite = self.session.as_ref().and_then(JamSession::share_url);
        let invite = session.as_ref().and_then(JamSession::share_url);
        if previous_invite != invite {
            self.qr = invite.as_deref().map(terminal_qr).unwrap_or_default();
        }
        self.session = session;
        self.known = true;
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.selected_member = overlay.selected_member.min(
                self.session
                    .as_ref()
                    .map_or(0, |s| s.members.len().saturating_sub(1)),
            );
        }
    }
}

pub(crate) struct JamOverlay {
    pub(crate) screen: JamScreen,
    pub(crate) join_input: tui_textarea::TextArea<'static>,
    pub(crate) participation: JamType,
    pub(crate) selected_member: usize,
}

impl Default for JamOverlay {
    fn default() -> Self {
        Self {
            screen: JamScreen::Overview,
            join_input: tui_textarea::TextArea::default(),
            participation: JamType::InPerson,
            selected_member: 0,
        }
    }
}

impl JamOverlay {
    pub(crate) fn query(&self) -> &str {
        self.join_input.lines().first().map_or("", String::as_str)
    }

    pub(crate) fn clear_query(&mut self) {
        self.join_input = tui_textarea::TextArea::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JamScreen {
    Overview,
    Join,
}

#[derive(Debug, Clone)]
pub(crate) enum JamOperation {
    Refresh,
    Start,
    Join {
        invite: String,
        participation: JamType,
    },
    End {
        session_id: String,
    },
    Leave {
        session_id: String,
    },
    Kick {
        session_id: String,
        member_id: String,
    },
    QueueOnly(bool),
    ParticipantVolume(bool),
}

impl JamOperation {
    pub(crate) fn pending_label(&self) -> &'static str {
        match self {
            Self::Refresh => "refreshing Jam…",
            Self::Start => "starting Jam…",
            Self::Join { .. } => "joining Jam…",
            Self::End { .. } => "ending Jam…",
            Self::Leave { .. } => "leaving Jam…",
            Self::Kick { .. } => "removing guest…",
            Self::QueueOnly(_) | Self::ParticipantVolume(_) => "updating Jam controls…",
        }
    }

    pub(crate) fn success_label(&self) -> &'static str {
        match self {
            Self::Refresh => "",
            Self::Start => "Jam started",
            Self::Join { .. } => "joined Jam",
            Self::End { .. } => "Jam ended",
            Self::Leave { .. } => "left Jam",
            Self::Kick { .. } => "guest removed",
            Self::QueueOnly(_) | Self::ParticipantVolume(_) => "Jam controls updated",
        }
    }
}

pub(crate) struct JamReply {
    pub(crate) operation: JamOperation,
    pub(crate) result: Result<Option<JamSession>, String>,
}

fn terminal_qr(invite: &str) -> Vec<String> {
    if invite.is_empty() {
        return Vec::new();
    }
    std::process::Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "4", "-o", "-", invite])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|qr| qr.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Cross-cutting session bookkeeping: which metadata fetch is still in flight
/// and the input timestamps that make Ctrl-C and double-click work.
pub(crate) struct SessionState {
    pub(crate) restore_uri: Option<String>,
    pub(crate) restore_on_startup: bool,
    // Track URI whose metadata was last requested. Fetches run on separate
    // blocking tasks and can land out of order when skipping quickly, so a
    // reply for any other track is stale and must be dropped.
    pub(crate) pending_meta: Option<String>,
    // Whether we reclaimed a live server-side session (vs. local fallback).
    pub(crate) reclaimed: bool,
    // `Some` while the access-point watchdog is reconnecting. The value
    // remembers whether playback was running before the first retry; repeated
    // reconnect notices must not overwrite that intent after we freeze the UI.
    pub(crate) resume_after_reconnect: Option<bool>,
    // Timestamp of last Ctrl-C — a second press within 1.5s quits.
    pub(crate) last_ctrl_c: Option<Instant>,
    pub(crate) last_click: Option<(u16, Instant)>,
}
