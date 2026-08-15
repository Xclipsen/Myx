//! Playlist catalogue fallback through Spotify's internal context resolver.
//!
//! Since February 2026 the Web API only returns items for playlists owned by
//! the current user (or playlists they collaborate on). Playback still has to
//! resolve public foreign playlists, though, so librespot's context endpoint is
//! the appropriate fallback for the TUI drill-in.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use librespot_core::{Session, SpotifyUri};
use tokio::task::JoinSet;

const MAX_TRACKS: usize = 1_000;
const METADATA_WORKERS: usize = 2;
const METADATA_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistTrack {
    pub uri: String,
    pub name: String,
    pub artist: String,
}

/// Resolve a playlist and its track labels without the restricted Web API.
///
/// The context endpoint gives us the complete, ordered URI list. Track labels
/// live in a separate protobuf endpoint, so those small reads run concurrently
/// and are cached by URI. Keeping the worker count bounded avoids a request
/// burst on very large playlists.
pub async fn playlist_tracks(session: &Session, context_uri: &str) -> Result<Vec<PlaylistTrack>> {
    let context = session
        .spclient()
        .get_context(context_uri)
        .await
        .context("resolve playlist context")?;
    let uris: Vec<String> = context
        .pages
        .iter()
        .flat_map(|page| page.tracks.iter())
        .filter_map(|track| track.uri.clone())
        .filter(|uri| uri.starts_with("spotify:track:"))
        .take(MAX_TRACKS)
        .collect();

    if uris.is_empty() {
        bail!("playlist context has no tracks");
    }

    let mut resolved = vec![None; uris.len()];
    let mut jobs = JoinSet::new();
    let mut next = 0;

    while next < uris.len() || !jobs.is_empty() {
        while next < uris.len() && jobs.len() < METADATA_WORKERS {
            let index = next;
            let uri = uris[index].clone();
            let session = session.clone();
            jobs.spawn(async move { (index, resolve_track(&session, uri).await) });
            next += 1;
        }

        if let Some(Ok((index, track))) = jobs.join_next().await {
            resolved[index] = track;
        }
    }

    let tracks: Vec<_> = resolved.into_iter().flatten().collect();
    if tracks.is_empty() {
        bail!("playlist track metadata unavailable");
    }
    Ok(tracks)
}

async fn resolve_track(session: &Session, uri: String) -> Option<PlaylistTrack> {
    let cache_key = format!("librespot:track-metadata:{uri}");
    if let Some(bytes) = crate::httpcache::get_bytes(&cache_key) {
        if let Some((name, artist)) = decode_track_metadata(&bytes) {
            return Some(PlaylistTrack { uri, name, artist });
        }
    }

    let spotify_uri = SpotifyUri::from_uri(&uri).ok()?;
    for attempt in 0..2 {
        if let Ok(Ok(bytes)) = tokio::time::timeout(
            METADATA_TIMEOUT,
            session.spclient().get_track_metadata(&spotify_uri),
        )
        .await
        {
            if let Some((name, artist)) = decode_track_metadata(bytes.as_ref()) {
                crate::httpcache::put_bytes(&cache_key, bytes.as_ref());
                return Some(PlaylistTrack { uri, name, artist });
            }
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    None
}

/// Decode only the fields the list needs from `spotify.metadata.Track`:
/// field 2 is the title and repeated field 4 contains Artist messages whose
/// field 2 is the name. No protobuf dependency is needed for these two stable
/// scalar fields.
fn decode_track_metadata(bytes: &[u8]) -> Option<(String, String)> {
    let mut title = None;
    let mut artists = Vec::new();
    let mut role_artists = Vec::new();

    visit_fields(bytes, |field, wire, value| {
        if wire != 2 {
            return;
        }
        match field {
            2 => title = std::str::from_utf8(value).ok().map(str::to_owned),
            4 => {
                if let Some(name) = string_field(value, 2) {
                    artists.push(name);
                }
            }
            // Newer metadata can carry the display names as ArtistWithRole.
            32 => {
                if let Some(name) = string_field(value, 2) {
                    role_artists.push(name);
                }
            }
            _ => {}
        }
    })?;

    let title = title.filter(|name| !name.is_empty())?;
    if artists.is_empty() {
        artists = role_artists;
    }
    artists.dedup();
    Some((title, artists.join(", ")))
}

fn string_field(bytes: &[u8], wanted: u64) -> Option<String> {
    let mut found = None;
    visit_fields(bytes, |field, wire, value| {
        if field == wanted && wire == 2 {
            found = std::str::from_utf8(value).ok().map(str::to_owned);
        }
    })?;
    found
}

/// Minimal protobuf wire walker. Length-delimited values are handed to the
/// callback; all other supported wire values are merely skipped.
fn visit_fields(mut bytes: &[u8], mut visit: impl FnMut(u64, u8, &[u8])) -> Option<()> {
    while !bytes.is_empty() {
        let (key, key_len) = read_varint(bytes)?;
        bytes = bytes.get(key_len..)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        if field == 0 {
            return None;
        }

        match wire {
            0 => {
                let (_, len) = read_varint(bytes)?;
                bytes = bytes.get(len..)?;
            }
            1 => bytes = bytes.get(8..)?,
            2 => {
                let (len, prefix) = read_varint(bytes)?;
                let len = usize::try_from(len).ok()?;
                bytes = bytes.get(prefix..)?;
                let value = bytes.get(..len)?;
                visit(field, wire, value);
                bytes = bytes.get(len..)?;
            }
            5 => bytes = bytes.get(4..)?,
            _ => return None,
        }
    }
    Some(())
}

fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().take(10).enumerate() {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(number: u8, value: &[u8]) -> Vec<u8> {
        let mut out = vec![(number << 3) | 2, value.len() as u8];
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn decodes_title_and_artists() {
        let mut bytes = field(2, b"Song");
        bytes.extend(field(4, &field(2, b"Artist One")));
        bytes.extend(field(4, &field(2, b"Artist Two")));
        assert_eq!(
            decode_track_metadata(&bytes),
            Some(("Song".to_string(), "Artist One, Artist Two".to_string()))
        );
    }

    #[test]
    fn malformed_metadata_is_rejected() {
        assert_eq!(decode_track_metadata(&[0x12, 0x7f, b'x']), None);
        assert_eq!(decode_track_metadata(&field(4, &field(2, b"Artist"))), None);
    }
}
