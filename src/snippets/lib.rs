//! Snippet storage shared by launch-gui and launch-snippetd. No GTK.
//!
//! File: one snippet per line (default ~/.config/launch-gui/snippets)
//!
//!   \p name | trigger = content          plain
//!   \e name | trigger = <base64 blob>    encrypted (ChaCha20-Poly1305)
//!   \p name = content                    no trigger (launcher-only)
//!   # comment
//!
//! Content escapes: `\n` newline, `\\` backslash.
//! Encryption key = blake3::derive_key(secret); the secret lives in
//! gnome-keyring (secret-tool), never on disk.

use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const KDF_CONTEXT: &str = "launch-gui snippets v1 2026-10-09";
const KEYRING_ATTRS: [&str; 4] = ["service", "launch-gui", "key", "snippets"];

// ---------------------------------------------------------------------------
// Settings ([snippets] section of ~/.config/launch-gui/config)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Settings {
    pub enabled: bool,
    /// Launcher prefix that lists snippets, e.g. "s'"
    pub prefix: String,
    pub file: PathBuf,
    /// launch-snippetd: expand triggers typed in any app
    pub expand: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("launch-gui")
}

fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(s),
    }
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            prefix: "s'".into(),
            file: config_dir().join("snippets"),
            expand: false,
        }
    }
}

impl Settings {
    /// Parse only the [snippets] section of the launcher config.
    pub fn load() -> Self {
        let mut s = Self::default();
        let Ok(content) = std::fs::read_to_string(config_dir().join("config")) else {
            return s;
        };
        let mut in_section = false;
        for line in content.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if t.starts_with('[') && t.ends_with(']') {
                in_section = t[1..t.len() - 1].trim().eq_ignore_ascii_case("snippets");
                continue;
            }
            if !in_section {
                continue;
            }
            let Some((k, v)) = t.split_once('=') else { continue };
            let v = unquote(v);
            match k.trim().to_lowercase().as_str() {
                "enabled" => s.enabled = matches!(v, "true" | "yes" | "1" | "on"),
                "prefix" => s.prefix = v.to_string(),
                "file" => s.file = expand_tilde(v),
                "expand" => s.expand = matches!(v, "true" | "yes" | "1" | "on"),
                _ => {}
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Snippets
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Snippet {
    pub name: String,
    pub trigger: Option<String>,
    pub encrypted: bool,
    /// Plain text, or the base64 blob when `encrypted`
    pub body: String,
}

impl Snippet {
    /// Plain content (decrypting if needed).
    pub fn content(&self, key: Option<&[u8; 32]>) -> Result<String, String> {
        if !self.encrypted {
            return Ok(self.body.clone());
        }
        let key = key.ok_or("no snippet key: run `launch-gui --snippet-key`")?;
        decrypt(key, &self.body)
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Parse one line: `\p name | trigger = content` / `\e ...`.
/// Also used for the launcher's "create snippet" input.
pub fn parse_line(line: &str) -> Option<Snippet> {
    let t = line.trim_start();
    let (encrypted, rest) = if let Some(r) = t.strip_prefix("\\p ") {
        (false, r)
    } else if let Some(r) = t.strip_prefix("\\e ") {
        (true, r)
    } else {
        return None;
    };
    let (head, body) = rest.split_once('=')?;
    let (name, trigger) = match head.split_once('|') {
        Some((n, t)) => (n.trim(), Some(t.trim()).filter(|t| !t.is_empty())),
        None => (head.trim(), None),
    };
    if name.is_empty() {
        return None;
    }
    let body = body.strip_prefix(' ').unwrap_or(body);
    Some(Snippet {
        name: name.to_string(),
        trigger: trigger.map(str::to_string),
        encrypted,
        body: if encrypted { body.trim().to_string() } else { unescape(body) },
    })
}

pub fn load(settings: &Settings) -> Vec<Snippet> {
    std::fs::read_to_string(&settings.file)
        .map(|c| c.lines().filter_map(parse_line).collect())
        .unwrap_or_default()
}

/// Append a snippet. `content` is plain text; encrypted snippets are
/// encrypted here. Same name replaces the old entry.
pub fn save(
    settings: &Settings,
    name: &str,
    trigger: Option<&str>,
    content: &str,
    encrypt_with: Option<&[u8; 32]>,
) -> Result<(), String> {
    let (tag, body) = match encrypt_with {
        Some(key) => ("\\e", encrypt(key, content)?),
        None => ("\\p", escape(content)),
    };
    let head = match trigger {
        Some(t) => format!("{name} | {t}"),
        None => name.to_string(),
    };
    let new_line = format!("{tag} {head} = {body}");

    let existing = std::fs::read_to_string(&settings.file).unwrap_or_default();
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| parse_line(l).is_none_or(|s| s.name != name))
        .collect();
    lines.push(&new_line);

    if let Some(dir) = settings.file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = settings.file.with_extension("tmp");
    std::fs::write(&tmp, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    set_private(&tmp);
    std::fs::rename(&tmp, &settings.file).map_err(|e| e.to_string())
}

/// 0600: the file can hold secrets (even if encrypted)
fn set_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

// ---------------------------------------------------------------------------
// Crypto
// ---------------------------------------------------------------------------

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut buf = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

pub fn derive_key(secret: &str) -> [u8; 32] {
    blake3::derive_key(KDF_CONTEXT, secret.as_bytes())
}

/// base64(nonce(12) || ciphertext+tag)
pub fn encrypt(key: &[u8; 32], plain: &str) -> Result<String, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = random_bytes::<12>()?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
        .map_err(|_| "encryption failed".to_string())?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ct);
    Ok(STANDARD.encode(blob))
}

pub fn decrypt(key: &[u8; 32], blob: &str) -> Result<String, String> {
    let raw = STANDARD.decode(blob.trim()).map_err(|_| "corrupt snippet".to_string())?;
    if raw.len() < 12 + 16 {
        return Err("corrupt snippet".into());
    }
    let (nonce, ct) = raw.split_at(12);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| "wrong key or tampered snippet".to_string())?;
    String::from_utf8(pt).map_err(|_| "snippet is not utf-8".into())
}

// ---------------------------------------------------------------------------
// Keyring (gnome-keyring via secret-tool)
// ---------------------------------------------------------------------------

/// Derived key from the secret stored in the keyring, if any.
pub fn key_from_keyring() -> Option<[u8; 32]> {
    let out = Command::new("secret-tool")
        .arg("lookup")
        .args(KEYRING_ATTRS)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let secret = String::from_utf8(out.stdout).ok()?;
    let secret = secret.trim_end_matches('\n');
    (!secret.is_empty()).then(|| derive_key(secret))
}

pub fn store_secret(secret: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .args(["store", "--label", "launch-gui snippets"])
        .args(KEYRING_ATTRS)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("secret-tool: {e} (install libsecret)"))?;
    if let Some(mut si) = child.stdin.take() {
        si.write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        _ => Err("secret-tool store failed (is gnome-keyring unlocked?)".into()),
    }
}

/// Read a line from the terminal without echo.
pub fn prompt_hidden(prompt: &str) -> Option<String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let fd = libc::STDIN_FILENO;
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    let is_tty = unsafe { libc::tcgetattr(fd, &mut old) } == 0;
    if is_tty {
        let mut new = old;
        new.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &new) };
    }
    let mut line = String::new();
    let ok = std::io::stdin().read_line(&mut line).is_ok();
    if is_tty {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
        eprintln!();
    }
    ok.then(|| line.trim_end_matches(['\n', '\r']).to_string())
}
