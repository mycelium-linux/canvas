//! Deadwax catalog adapter. Canvas never opens the catalog database or scans music paths.
use serde::Deserialize;

/// Stable selection tuple, independent of Deadwax's derived track row IDs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct Album {
    pub artist: String,
    pub album: String,
    pub year: Option<u16>,
    pub tracks: usize,
}

/// Catalog track metadata; paths deliberately stay on the owner side.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct Track {
    pub title: String,
    pub position: String,
    pub artist: String,
    pub album: String,
}

/// Owner acknowledgement; IDs belong to MPD's queue, not catalog rows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct SelectionReceipt {
    pub song_ids: Vec<u32>,
    pub started: bool,
}

/// Bounded blocking catalog client, intended for an acquisition worker, never rendering.
pub struct Library {
    client: reqwest::blocking::Client,
    base: reqwest::Url,
    bearer: Option<String>,
}

impl Library {
    /// Fetch authoritative local artwork. Missing art is distinct from a failed request.
    pub fn artwork(&self, album: &Album) -> Result<Option<Vec<u8>>, String> {
        use std::io::Read;
        let mut url = self
            .base
            .join("albums/art")
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("artist", &album.artist)
            .append_pair("album", &album.album);
        let mut request = self.client.get(url);
        if let Some(token) = &self.bearer {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|_| "artwork request failed".to_string())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .map_err(|_| "artwork owner failed".to_string())?;
        const MAX: u64 = 8 * 1024 * 1024;
        let mut bytes = Vec::new();
        response
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "artwork read failed".to_string())?;
        if bytes.len() as u64 > MAX {
            return Err("artwork exceeds 8 MiB".into());
        }
        Ok(Some(bytes))
    }
    pub fn new(base: &str, bearer: Option<String>) -> Result<Self, String> {
        let base = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(
                "catalog requires an HTTP(S) origin without credentials, path, query or fragment"
                    .into(),
            );
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            base,
            bearer,
        })
    }

    /// Load one bounded page. No call here changes playback.
    pub fn albums(&self, offset: usize) -> Result<Vec<Album>, String> {
        self.albums_by_artist(offset, "")
    }

    /// Exact artist scope; an empty artist selects all locally downloaded albums.
    pub fn albums_by_artist(&self, offset: usize, artist: &str) -> Result<Vec<Album>, String> {
        let mut url = self
            .base
            .join("albums")
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("limit", "24")
            .append_pair("offset", &offset.to_string());
        if !artist.is_empty() { url.query_pairs_mut().append_pair("artist", artist); }
        let albums: Vec<Album> = self.get(url)?;
        if albums.len() > 24 {
            return Err("catalog exceeded requested album page size".into());
        }
        Ok(albums)
    }

    /// Bounded artist page from the catalog owner.
    pub fn artists(&self, offset: usize) -> Result<Vec<String>, String> {
        let mut url = self.base.join("artists").map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("limit", "24").append_pair("offset", &offset.to_string());
        let values: Vec<String> = self.get(url)?;
        if values.len() > 24 || values.iter().any(|value| value.len() > 8192) { return Err("Invalid artist page".into()); }
        Ok(values)
    }

    /// Imported playlist metadata; local playability must not be inferred from membership.
    pub fn playlists(&self, offset: usize) -> Result<serde_json::Value, String> {
        let mut url=self.base.join("playlists").map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("summary","1").append_pair("limit","24").append_pair("offset",&offset.to_string());
        let values: serde_json::Value = self.get(url)?;
        let values = values.as_array().ok_or("Invalid playlist catalog")?;
        if values.len()>24 || values.iter().any(|value|value["playlist"].get("tracks").is_some()) {return Err("Expected bounded playlist summaries".into());}
        Ok(serde_json::Value::Array(values.clone()))
    }

    /// A bounded metadata preview, without resolving provider tracks or changing playback.
    pub fn playlist_tracks(&self, owner:&str,name:&str)->Result<serde_json::Value,String> {
        let mut url=self.base.join("playlists/tracks").map_err(|e|e.to_string())?;
        url.query_pairs_mut().append_pair("owner",owner).append_pair("name",name).append_pair("limit","100");
        let value:serde_json::Value=self.get(url)?;
        if value.as_array().is_none_or(|tracks|tracks.len()>100) {return Err("Invalid playlist preview".into());}
        Ok(value)
    }

    pub fn tracks(&self, album: &Album) -> Result<Vec<Track>, String> {
        let url = self.tracks_url(album)?;
        let tracks: Vec<Track> = self.get(url)?;
        if tracks.len() > 4096
            || tracks
                .iter()
                .any(|track| track.artist != album.artist || track.album != album.album)
        {
            return Err("catalog returned invalid album membership".into());
        }
        Ok(tracks)
    }

    /// Explicit user selection only. Never invoke while refreshing/rendering the shelf.
    /// Starting retains the existing shared MPD queue; enqueue does not start playback.
    pub fn select(&self, album: &Album, start: bool) -> Result<SelectionReceipt, String> {
        let mut url = self
            .base
            .join("queue/album")
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("artist", &album.artist)
            .append_pair("album", &album.album)
            .append_pair("action", if start { "play" } else { "enqueue" });
        let mut request = self.client.post(url);
        if let Some(token) = &self.bearer {
            request = request.bearer_auth(token);
        }
        let receipt: SelectionReceipt = Self::response(request)?;
        if receipt.song_ids.is_empty() || receipt.song_ids.len() > 256 || receipt.started != start {
            return Err("invalid playback selection acknowledgement".into());
        }
        Ok(receipt)
    }

    fn tracks_url(&self, album: &Album) -> Result<reqwest::Url, String> {
        let mut url = self
            .base
            .join("albums/tracks")
            .map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("artist", &album.artist)
            .append_pair("album", &album.album);
        Ok(url)
    }

    fn get<T: serde::de::DeserializeOwned>(&self, url: reqwest::Url) -> Result<T, String> {
        let mut request = self.client.get(url);
        if let Some(token) = &self.bearer {
            request = request.bearer_auth(token);
        }
        Self::response(request)
    }

    fn response<T: serde::de::DeserializeOwned>(
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<T, String> {
        use std::io::Read;
        let response = request
            .send()
            .map_err(|_| "catalog request failed".to_string())?
            .error_for_status()
            .map_err(|error| format!("catalog HTTP status: {:?}", error.status()))?;
        const MAX_BYTES: u64 = 1024 * 1024;
        let mut bytes = Vec::new();
        response
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "catalog read failed".to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("catalog response exceeds 1 MiB".into());
        }
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid catalog JSON: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock(body: &str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            request
        });
        (origin, handle)
    }

    #[test]
    fn browse_is_read_only_and_does_not_request_paths() {
        let (origin, handle) =
            mock(r#"[{"artist":"Artist","album":"Album","year":2020,"tracks":2}]"#);
        let library = Library::new(&origin, None).unwrap();
        assert_eq!(library.albums(24).unwrap()[0].album, "Album");
        assert_eq!(
            handle.join().unwrap(),
            "GET /albums?limit=24&offset=24 HTTP/1.1\r\n"
        );
    }

    #[test]
    fn artist_browse_is_bounded_and_exact_scope_is_encoded() {
        let (origin, handle) = mock(r#"["A & B"]"#);
        let library = Library::new(&origin, None).unwrap();
        assert_eq!(library.artists(24).unwrap(), vec!["A & B"]);
        assert_eq!(handle.join().unwrap(), "GET /artists?limit=24&offset=24 HTTP/1.1\r\n");
        let (origin, handle) = mock("[]");
        Library::new(&origin, None).unwrap().albums_by_artist(0, "A & B").unwrap();
        assert_eq!(handle.join().unwrap(), "GET /albums?limit=24&offset=0&artist=A+%26+B HTTP/1.1\r\n");
    }

    #[test]
    fn playlists_are_paged_without_claiming_local_playability() {
        let (origin, handle) = mock(r#"[{"owner":"me","playlist":{"name":"One","track_count":10}}]"#);
        assert_eq!(Library::new(&origin, None).unwrap().playlists(0).unwrap()[0]["playlist"]["name"], "One");
        assert_eq!(handle.join().unwrap(), "GET /playlists?summary=1&limit=24&offset=0 HTTP/1.1\r\n");
    }

    #[test]
    fn explicit_enqueue_uses_post_and_requires_owner_receipt() {
        let (origin, handle) = mock(r#"{"song_ids":[42,43],"started":false}"#);
        let library = Library::new(&origin, None).unwrap();
        let album = Album {
            artist: "A & B".into(),
            album: "Album".into(),
            year: None,
            tracks: 2,
        };
        assert_eq!(
            library.select(&album, false).unwrap().song_ids,
            vec![42, 43]
        );
        let request = handle.join().unwrap();
        assert!(request.starts_with("POST /queue/album?"));
        assert!(request.contains("artist=A+%26+B&album=Album&action=enqueue"));
    }

    #[test]
    fn malformed_owner_receipt_is_not_success() {
        let (origin, handle) = mock(r#"{"song_ids":[],"started":true}"#);
        let library = Library::new(&origin, None).unwrap();
        let album = Album {
            artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            tracks: 1,
        };
        assert!(library.select(&album, true).is_err());
        handle.join().unwrap();
    }
    #[test]
    fn rejects_credentials_and_non_origin_urls() {
        for url in [
            "file:///srv/music",
            "http://user:password@localhost/",
            "http://localhost/api",
            "http://localhost/?x=1",
        ] {
            assert!(Library::new(url, None).is_err());
        }
    }
    #[test]
    fn exact_identity_is_encoded_not_interpolated() {
        let library = Library::new("http://127.0.0.1:6601", None).unwrap();
        let album = Album {
            artist: "A & B".into(),
            album: "Live / ? #".into(),
            year: None,
            tracks: 3,
        };
        let url = library.tracks_url(&album).unwrap();
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs["artist"], album.artist);
        assert_eq!(pairs["album"], album.album);
        assert_eq!(url.path(), "/albums/tracks");
    }
}
