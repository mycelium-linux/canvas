//! The private loopback credential Canvas presents to FPL Desktop Bridge.
//!
//! Canvas serves two MCP surfaces on this machine — presentation from `canvas`
//! and workspace orchestration from `canvas-mac` — and both are the same
//! application, on the same account, behind the same loopback boundary. They
//! therefore read one credential file rather than minting one each: a second
//! token would double what has to be rotated and audited without drawing a
//! boundary anywhere, and the two would drift the first time only one was
//! reissued.
//!
//! The file is the boundary, so the checks on it are the security-relevant part
//! and live here once. A bearer on loopback is not authentication of a person;
//! OS account permissions are. This only ensures a process that is not this user
//! cannot read the token out of a world-readable file or a swapped symlink.

use std::path::PathBuf;

/// Where both MCP servers read the shared loopback bearer.
///
/// The path still says `Dock`: it names a file already installed on this machine
/// and referenced by the Desktop Bridge connector config, so it moves in a
/// deliberate migration rather than a rename. See the naming note in `AGENTS.md`.
pub fn token_path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/mcp/token"),
    )
}

/// Read the credential, refusing anything another account could have influenced.
///
/// `O_NOFOLLOW` matters as much as the mode check: without it, replacing the
/// token with a symlink into a file this user can read turns a permission check
/// on the link into a read of the target.
pub fn local_token() -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(token_path()?)
        .map_err(|_| "Private Canvas MCP credential unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect Canvas MCP credential")?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("Canvas MCP credential must be a private file owned by this user".into());
    }
    let mut token = String::new();
    // One byte past the longest credential this accepts, so an over-long file is
    // rejected by the length check below rather than silently truncated into a
    // valid-looking prefix.
    file.by_ref()
        .take(129)
        .read_to_string(&mut token)
        .map_err(|_| "Cannot read Canvas MCP credential")?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid Canvas MCP credential".into());
    }
    Ok(token)
}

/// Compare a presented bearer against the credential in constant time.
///
/// Ordinary `==` returns as soon as two bytes differ, which leaks the length of
/// the matching prefix to anything that can time the reply — enough to recover
/// the token one byte at a time over a loopback socket that answers fast.
pub fn token_matches(provided: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    bool::from(provided.as_bytes().ct_eq(expected.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_path_is_the_one_the_desktop_bridge_config_points_at() {
        // Both MCP servers and the installed bridge connector must name one file;
        // a change here silently unauthenticates whichever side was not updated.
        let path = token_path().expect("HOME is set under test");
        assert!(
            path.ends_with("Library/Application Support/Dock/mcp/token"),
            "{path:?}"
        );
    }

    #[test]
    fn a_bearer_matches_only_itself() {
        let token = "a".repeat(64);
        assert!(token_matches(&token, &token));
        assert!(!token_matches(&token, &"a".repeat(63)));
        assert!(!token_matches("", &token));
        // A correct prefix is not a correct token; this is the case a timing
        // attack builds up one byte at a time.
        assert!(!token_matches(&"a".repeat(63), &token));
    }
}
