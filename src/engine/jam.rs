//! Experimental Spotify Jam support over the private social-connect service.
//!
//! Spotify does not expose Jam through the public Web API.  Keep every private
//! endpoint in this module so a protocol change has one small blast radius and
//! the rest of Myx only deals with normalized session data.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use librespot_core::dealer::protocol::PayloadValue;
use librespot_core::Session;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{json, Value};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const CURRENT: &str = "/social-connect/v2/sessions/current?alt=protobuf";
const CURRENT_OR_NEW: &str = "/social-connect/v2/sessions/current_or_new?activate=";
const V2_SESSIONS: &str = "/social-connect/v2/sessions/";
const V3_SESSIONS: &str = "/social-connect/v3/sessions/";
const PUBLIC_SESSION_URL: &str = "https://open.spotify.com/socialsession/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JamType {
    InPerson,
    Remote,
    RemoteV2,
    #[default]
    Unknown,
}

impl JamType {
    pub fn label(self) -> &'static str {
        match self {
            Self::InPerson => "in person",
            Self::Remote | Self::RemoteV2 => "remote",
            Self::Unknown => "unknown",
        }
    }

    fn command_value(self) -> &'static str {
        match self {
            Self::Remote | Self::RemoteV2 => "remote",
            Self::InPerson | Self::Unknown => "in_person",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JamMember {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub image_url: String,
    pub is_listening: bool,
    pub is_controlling: bool,
}

impl JamMember {
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JamSession {
    pub session_id: String,
    pub join_token: String,
    pub join_url: String,
    pub join_uri: String,
    pub owner_id: String,
    pub members: Vec<JamMember>,
    pub is_owner: bool,
    pub is_listening: bool,
    pub is_controlling: bool,
    pub is_discoverable: bool,
    pub session_type: JamType,
    pub host_device_id: String,
    /// `true` means guests may only append to the queue.
    pub queue_only: Option<bool>,
    /// `None` also covers devices on which Spotify does not offer shared volume.
    pub participant_volume: Option<bool>,
}

impl JamSession {
    /// Return an invite that can be opened outside Spotify's private protocol.
    ///
    /// Some Spotify responses expose only an `hm://social-connect/...` route.
    /// That route is useful internally but cannot be opened by a phone camera.
    pub fn share_url(&self) -> Option<String> {
        if let Some(url) = [&self.join_url, &self.join_uri]
            .into_iter()
            .map(|value| value.trim())
            .find(|value| value.starts_with("https://") || value.starts_with("http://"))
        {
            return Some(url.to_string());
        }

        let invite = [&self.join_token, &self.join_uri, &self.join_url]
            .into_iter()
            .find(|value| !value.trim().is_empty())?;
        let token = invite_token(invite).ok()?;
        Some(format!("{PUBLIC_SESSION_URL}{}", segment(&token).ok()?))
    }

    pub fn owner(&self) -> Option<&JamMember> {
        self.members
            .iter()
            .find(|member| member.id == self.owner_id)
    }

    pub fn merge_known_controls_from(&mut self, previous: &Self) {
        if self.session_id != previous.session_id {
            return;
        }
        self.queue_only = self.queue_only.or(previous.queue_only);
        self.participant_volume = self.participant_volume.or(previous.participant_volume);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JamUpdate {
    pub reason: String,
    pub session: Option<JamSession>,
}

/// Subscribe beside Spirc. Dealer subscriptions fan out, so this does not take
/// the update away from librespot's own session-id handling. The callback keeps
/// dealer stream types out of `engine/mod.rs`.
pub(crate) fn subscribe_with(
    session: &Session,
    mut send: impl FnMut(JamUpdate) -> bool + Send + 'static,
) -> Result<impl std::future::Future<Output = ()>> {
    let mut updates = session
        .dealer()
        .listen_for("social-connect/v2/session_update", |message| {
            let payload = match message.payload {
                PayloadValue::Json(json) => json,
                PayloadValue::Raw(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                PayloadValue::Empty => String::new(),
            };
            Ok(payload)
        })
        .context("subscribe to Jam session updates")?;

    Ok(async move {
        while let Some(update) = std::future::poll_fn(|cx| updates.as_mut().poll_next(cx)).await {
            let Ok(payload) = update else { break };
            if let Ok(update) = update_from_json(&payload) {
                if !send(update) {
                    break;
                }
            }
        }
    })
}

pub async fn current(session: &Session) -> Result<Option<JamSession>> {
    match request(session, Method::GET, CURRENT, None, None).await {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => decode_session(&bytes).map(Some),
        Err(error) if is_missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn start(session: &Session) -> Result<JamSession> {
    let endpoint = format!("{CURRENT_OR_NEW}true");
    let bytes = request(session, Method::GET, &endpoint, None, None).await?;
    if bytes.is_empty() {
        return current(session)
            .await?
            .ok_or_else(|| anyhow!("Spotify created no Jam session"));
    }
    decode_session(&bytes)
}

pub async fn end(session: &Session, session_id: &str) -> Result<()> {
    let endpoint = format!("{V3_SESSIONS}{}", segment(session_id)?);
    request(session, Method::DELETE, &endpoint, None, None)
        .await
        .map(|_| ())
}

pub async fn leave(session: &Session, session_id: &str) -> Result<()> {
    let endpoint = format!(
        "{V3_SESSIONS}{}/leave?reason=user_initiated",
        segment(session_id)?
    );
    request(session, Method::POST, &endpoint, None, None)
        .await
        .map(|_| ())
}

pub async fn kick(session: &Session, session_id: &str, member_id: &str) -> Result<()> {
    let endpoint = format!(
        "{V3_SESSIONS}{}/member/{}/kick",
        segment(session_id)?,
        segment(member_id)?
    );
    request(session, Method::POST, &endpoint, None, None)
        .await
        .map(|_| ())
}

pub async fn set_queue_only(session: &Session, enabled: bool) -> Result<()> {
    let value = if enabled { "enabled" } else { "disabled" };
    let endpoint = format!("/social-connect/v2/sessions/current/queue_only_mode/{value}");
    request(session, Method::PUT, &endpoint, None, None)
        .await
        .map(|_| ())
}

pub async fn set_participant_volume(session: &Session, enabled: bool) -> Result<()> {
    let value = if enabled { "enabled" } else { "disabled" };
    let endpoint = format!("/social-connect/v2/sessions/current/volume_control/{value}");
    request(session, Method::PUT, &endpoint, None, None)
        .await
        .map(|_| ())
}

pub async fn session_info(session: &Session, invite: &str) -> Result<JamSession> {
    let token = invite_token(invite)?;
    let endpoint = format!("{V2_SESSIONS}{}?alt=protobuf", segment(&token)?);
    let bytes = request(session, Method::GET, &endpoint, None, None).await?;
    decode_session(&bytes)
}

/// Ask the active Jam host to admit this Connect device.  This command path is
/// private and has changed before, so success is never inferred from HTTP 2xx:
/// we wait until the current-session endpoint confirms membership.
pub async fn join(session: &Session, invite: &str, participation: JamType) -> Result<JamSession> {
    let token = invite_token(invite)?;
    let info = session_info(session, &token).await?;
    if let Some(joined) = confirmed_membership(session, &info.session_id, 1).await? {
        return Ok(joined);
    }

    // Older and some mobile-created sessions advertise an hm:// join endpoint
    // directly in the session. Prefer the server-provided route when present;
    // current desktop sessions instead use the command bridge below.
    if let Some(join_endpoint) = [&info.join_url, &info.join_uri]
        .into_iter()
        .find(|value| value.starts_with("hm://"))
    {
        let legacy = session.mercury().get(join_endpoint.to_string());
        if let Ok(request) = legacy {
            if let Ok(Ok(response)) = tokio::time::timeout(REQUEST_TIMEOUT, request).await {
                if response.status_code == 200 {
                    if let Some(joined) = confirmed_membership(session, &info.session_id, 4).await?
                    {
                        return Ok(joined);
                    }
                }
            }
        }
    }

    let device_id = session.device_id().to_string();
    let message_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;
    let body = json!({
        "message_id": message_id,
        "sent_by_device_id": device_id,
        "command": {
            "endpoint": "join_session",
            "session_id": info.session_id,
            "join_session_token": token,
            "join_type": "deeplinking",
            "participation_mode": participation.command_value(),
            "logging_params": {
                "device_identifier": session.device_id(),
                "command_initiated_time": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            }
        }
    })
    .to_string();
    let endpoint = format!(
        "{V2_SESSIONS}{}/commands/from/{}",
        segment(&info.session_id)?,
        segment(session.device_id())?
    );
    let mut headers = HeaderMap::new();
    headers.insert("x-transfer-encoding", HeaderValue::from_static("command"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    request(
        session,
        Method::POST,
        &endpoint,
        Some(headers),
        Some(body.as_bytes()),
    )
    .await?;

    if let Some(joined) = confirmed_membership(session, &info.session_id, 10).await? {
        return Ok(joined);
    }
    bail!("Spotify accepted the join request but did not confirm membership")
}

async fn confirmed_membership(
    session: &Session,
    expected_session_id: &str,
    attempts: usize,
) -> Result<Option<JamSession>> {
    for attempt in 0..attempts {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
        if let Some(joined) = current(session).await? {
            if joined.session_id == expected_session_id {
                return Ok(Some(joined));
            }
        }
    }
    Ok(None)
}

async fn request(
    session: &Session,
    method: Method,
    endpoint: &str,
    headers: Option<HeaderMap>,
    body: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let response = tokio::time::timeout(
        REQUEST_TIMEOUT,
        session.spclient().request(&method, endpoint, headers, body),
    )
    .await
    .map_err(|_| anyhow!("Jam request timed out"))?
    .map_err(|error| anyhow!("Jam request failed: {error}"))?;
    Ok(response.to_vec())
}

fn is_missing(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains("404") || message.contains("not found")
}

fn segment(value: &str) -> Result<String> {
    if value.is_empty() {
        bail!("empty Jam identifier");
    }
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    Ok(encoded)
}

pub fn invite_token(invite: &str) -> Result<String> {
    let invite = invite.trim();
    if invite.is_empty() {
        bail!("paste a Jam link, URI or token first");
    }
    let without_query = invite.split(['?', '#']).next().unwrap_or(invite);
    let token = without_query
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(without_query)
        .trim();
    if token.is_empty() {
        bail!("could not read a Jam token from that invite");
    }
    Ok(token.to_string())
}

fn decode_session(bytes: &[u8]) -> Result<JamSession> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return session_from_value(&value)
            .ok_or_else(|| anyhow!("Spotify returned no Jam session"));
    }
    session_from_protobuf(bytes).context("decode Jam session")
}

pub fn update_from_json(payload: &str) -> Result<JamUpdate> {
    let value: Value = serde_json::from_str(payload).context("decode Jam update")?;
    let reason = string(&value, &["reason", "update_reason", "updateReason"]);
    let ended = matches!(
        reason.as_str(),
        "SESSION_DELETED" | "YOU_LEFT" | "YOU_WERE_KICKED"
    );
    Ok(JamUpdate {
        reason,
        session: (!ended).then(|| session_from_value(&value)).flatten(),
    })
}

fn session_from_value(value: &Value) -> Option<JamSession> {
    let value = object(value, &["session", "current_session", "currentSession"]).unwrap_or(value);
    if bool_value(value, &["active"]) == Some(false) {
        return None;
    }
    let session_id = string(value, &["session_id", "sessionId"]);
    if session_id.is_empty() {
        return None;
    }
    let members_value = array(value, &["session_members", "sessionMembers", "members"]);
    let members = members_value
        .into_iter()
        .flatten()
        .filter_map(member_from_value)
        .collect();
    let volume = value_at(
        value,
        &["participant_volume_control", "participantVolumeControl"],
    )
    .and_then(setting_bool);
    Some(JamSession {
        session_id,
        join_token: string(value, &["join_session_token", "joinSessionToken"]),
        join_url: string(
            value,
            &[
                "join_session_url",
                "joinSessionUrl",
                "join_session_short_link",
                "joinSessionShortLink",
            ],
        ),
        join_uri: string(value, &["join_session_uri", "joinSessionUri"]),
        owner_id: string(value, &["session_owner_id", "sessionOwnerId"]),
        members,
        is_owner: bool_value(value, &["is_session_owner", "isSessionOwner"]).unwrap_or(false),
        is_listening: bool_value(value, &["is_listening", "isListening"]).unwrap_or(false),
        is_controlling: bool_value(value, &["is_controlling", "isControlling"]).unwrap_or(false),
        is_discoverable: bool_value(value, &["is_discoverable", "isDiscoverable"]).unwrap_or(false),
        session_type: jam_type(value_at(
            value,
            &[
                "initial_session_type",
                "initialSessionType",
                "session_type",
                "sessionType",
            ],
        )),
        host_device_id: string(value, &["host_active_device_id", "hostActiveDeviceId"]),
        queue_only: value_at(value, &["queue_only_mode", "queueOnlyMode"]).and_then(setting_bool),
        participant_volume: volume,
    })
}

fn member_from_value(value: &Value) -> Option<JamMember> {
    let id = string(value, &["id"]);
    let username = string(value, &["username"]);
    if id.is_empty() && username.is_empty() {
        return None;
    }
    Some(JamMember {
        id,
        username,
        display_name: string(value, &["display_name", "displayName"]),
        image_url: string(value, &["image_url", "imageUrl"]),
        is_listening: bool_value(value, &["is_listening", "isListening"]).unwrap_or(false),
        is_controlling: bool_value(value, &["is_controlling", "isControlling"]).unwrap_or(false),
    })
}

fn value_at<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(*name))
}

fn object<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    value_at(value, names).filter(|value| value.is_object())
}

fn array<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Vec<Value>> {
    value_at(value, names).and_then(Value::as_array)
}

fn string(value: &Value, names: &[&str]) -> String {
    let Some(value) = value_at(value, names) else {
        return String::new();
    };
    if let Some(value) = value.as_str() {
        return value.to_string();
    }
    // The desktop bridge wraps short links in an object.
    if let Some(value) = value.as_object() {
        return ["shareableUrl", "url", "spotifyUri"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
    }
    String::new()
}

fn bool_value(value: &Value, names: &[&str]) -> Option<bool> {
    value_at(value, names).and_then(setting_bool)
}

fn setting_bool(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value.as_str().and_then(|value| match value {
            "ENABLED" | "enabled" | "true" => Some(true),
            "DISABLED" | "disabled" | "false" => Some(false),
            _ => None,
        })
    })
}

fn jam_type(value: Option<&Value>) -> JamType {
    match value {
        Some(Value::Number(number)) => match number.as_u64() {
            Some(3) => JamType::InPerson,
            Some(4) => JamType::Remote,
            Some(5) => JamType::RemoteV2,
            _ => JamType::Unknown,
        },
        Some(Value::String(value)) => match value.to_ascii_uppercase().as_str() {
            "IN_PERSON" => JamType::InPerson,
            "REMOTE" => JamType::Remote,
            "REMOTE_V2" => JamType::RemoteV2,
            _ => JamType::Unknown,
        },
        _ => JamType::Unknown,
    }
}

// Minimal protobuf reader for socialconnect.Session.  The schema is already a
// librespot transitive input, but decoding these few fields locally avoids a
// new direct production dependency solely for one private response.
fn session_from_protobuf(bytes: &[u8]) -> Result<JamSession> {
    let mut session = JamSession::default();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let key = varint(bytes, &mut cursor)?;
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (2, 2) => session.session_id = text_field(bytes, &mut cursor)?,
            (3, 2) => session.join_token = text_field(bytes, &mut cursor)?,
            (4, 2) => session.join_url = text_field(bytes, &mut cursor)?,
            (5, 2) => session.owner_id = text_field(bytes, &mut cursor)?,
            (6, 2) => {
                let member = bytes_field(bytes, &mut cursor)?;
                session.members.push(member_from_protobuf(member)?);
            }
            (7, 2) => session.join_uri = text_field(bytes, &mut cursor)?,
            (9, 0) => session.is_owner = varint(bytes, &mut cursor)? != 0,
            (10, 0) => session.is_listening = varint(bytes, &mut cursor)? != 0,
            (11, 0) => session.is_controlling = varint(bytes, &mut cursor)? != 0,
            (12, 0) => session.is_discoverable = varint(bytes, &mut cursor)? != 0,
            (13, 0) => {
                session.session_type = match varint(bytes, &mut cursor)? {
                    3 => JamType::InPerson,
                    4 => JamType::Remote,
                    5 => JamType::RemoteV2,
                    _ => JamType::Unknown,
                }
            }
            (14, 2) => session.host_device_id = text_field(bytes, &mut cursor)?,
            _ => skip_field(bytes, &mut cursor, wire)?,
        }
    }
    if session.session_id.is_empty() {
        bail!("Jam session response had no session id");
    }
    Ok(session)
}

fn member_from_protobuf(bytes: &[u8]) -> Result<JamMember> {
    let mut member = JamMember::default();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let key = varint(bytes, &mut cursor)?;
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (2, 2) => member.id = text_field(bytes, &mut cursor)?,
            (3, 2) => member.username = text_field(bytes, &mut cursor)?,
            (4, 2) => member.display_name = text_field(bytes, &mut cursor)?,
            (5, 2) => member.image_url = text_field(bytes, &mut cursor)?,
            (7, 0) => member.is_listening = varint(bytes, &mut cursor)? != 0,
            (8, 0) => member.is_controlling = varint(bytes, &mut cursor)? != 0,
            _ => skip_field(bytes, &mut cursor, wire)?,
        }
    }
    Ok(member)
}

fn varint(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| anyhow!("truncated protobuf varint"))?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("protobuf varint is too long")
}

fn bytes_field<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
    let len = usize::try_from(varint(bytes, cursor)?).context("protobuf length overflow")?;
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| anyhow!("truncated protobuf field"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn text_field(bytes: &[u8], cursor: &mut usize) -> Result<String> {
    Ok(std::str::from_utf8(bytes_field(bytes, cursor)?)
        .context("Jam field was not UTF-8")?
        .to_string())
}

fn skip_field(bytes: &[u8], cursor: &mut usize, wire: u64) -> Result<()> {
    match wire {
        0 => {
            let _ = varint(bytes, cursor)?;
        }
        1 => *cursor = (*cursor).saturating_add(8),
        2 => {
            let _ = bytes_field(bytes, cursor)?;
        }
        5 => *cursor = (*cursor).saturating_add(4),
        _ => bail!("unsupported protobuf wire type {wire}"),
    }
    if *cursor > bytes.len() {
        bail!("truncated protobuf field");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_parser_accepts_link_uri_and_token() {
        assert_eq!(invite_token("abc").unwrap(), "abc");
        assert_eq!(invite_token("spotify:jam:abc").unwrap(), "abc");
        assert_eq!(
            invite_token("https://spotify.link/abc?ref=jam").unwrap(),
            "abc"
        );
        assert!(invite_token("   ").is_err());
    }

    #[test]
    fn share_url_rewrites_private_invite() {
        let session = JamSession {
            join_token: "0mpO02FUoSgxHNhn3ZdKk6".to_string(),
            join_url: "hm://social-connect/v2/sessions/join/0mpO02FUoSgxHNhn3ZdKk6".to_string(),
            ..JamSession::default()
        };
        assert_eq!(
            session.share_url().as_deref(),
            Some("https://open.spotify.com/socialsession/0mpO02FUoSgxHNhn3ZdKk6")
        );
    }

    #[test]
    fn share_url_preserves_public_invite() {
        let session = JamSession {
            join_url: "https://spotify.link/abc".to_string(),
            ..JamSession::default()
        };
        assert_eq!(
            session.share_url().as_deref(),
            Some("https://spotify.link/abc")
        );
    }

    #[test]
    fn share_url_can_extract_token_from_uri() {
        let session = JamSession {
            join_uri: "spotify:socialsession:abc".to_string(),
            ..JamSession::default()
        };
        assert_eq!(
            session.share_url().as_deref(),
            Some("https://open.spotify.com/socialsession/abc")
        );
    }

    #[test]
    fn parses_camel_case_session_updates() {
        let update = update_from_json(
            r#"{"reason":"USER_JOINED","session":{"sessionId":"s1","joinSessionUrl":"https://spotify.link/a","sessionOwnerId":"owner","isSessionOwner":true,"initialSessionType":"IN_PERSON","sessionMembers":[{"id":"owner","username":"u","displayName":"Host","isListening":true}]}}"#,
        )
        .unwrap();
        let session = update.session.unwrap();
        assert_eq!(session.session_id, "s1");
        assert_eq!(session.session_type, JamType::InPerson);
        assert_eq!(session.owner().unwrap().label(), "Host");
    }

    #[test]
    fn terminal_updates_clear_the_session() {
        let update =
            update_from_json(r#"{"reason":"SESSION_DELETED","session":{"sessionId":"old"}}"#)
                .unwrap();
        assert!(update.session.is_none());
    }
}
