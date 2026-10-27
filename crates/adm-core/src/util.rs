//! Small, dependency-free helpers: response-header parsing, file names, atomic file publishing.

use percent_encoding::percent_decode_str;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// `bytes 0-999/1000` -> `(0, Some(999), Some(1000))`. `*` parts become `None`.
pub fn parse_content_range(v: &str) -> Option<(u64, Option<u64>, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let total = total.trim().parse::<u64>().ok();
    if range.trim() == "*" {
        return None;
    }
    let (s, e) = range.split_once('-')?;
    Some((s.trim().parse().ok()?, e.trim().parse().ok(), total))
}

/// The file name carried by a `Content-Disposition` header (RFC 6266), preferring `filename*`.
pub fn parse_content_disposition(v: &str) -> Option<String> {
    let mut plain = None;
    let mut ext = None;
    for (key, value) in disposition_params(v) {
        match key.to_ascii_lowercase().as_str() {
            "filename*" => ext = decode_ext_value(&value),
            "filename" => plain = Some(value),
            _ => {}
        }
    }
    ext.or(plain).filter(|s| !s.trim().is_empty())
}

/// Splits `attachment; filename="a;b.txt"; size=1` into `(key, value)` pairs, honouring quotes.
fn disposition_params(v: &str) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    let (mut cur, mut quoted, mut escaped) = (String::new(), false, false);
    for c in v.chars() {
        match c {
            _ if escaped => {
                cur.push(c);
                escaped = false;
            }
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
        .into_iter()
        .skip(1) // the disposition type itself
        .filter_map(|p| {
            let (k, val) = p.split_once('=')?;
            Some((k.trim().to_string(), val.trim().to_string()))
        })
        .collect()
}

/// `UTF-8''na%C3%AFve.txt` -> `naïve.txt`.
fn decode_ext_value(v: &str) -> Option<String> {
    let mut it = v.splitn(3, '\'');
    let charset = it.next()?;
    let _lang = it.next()?;
    let encoded = it.next()?;
    let bytes: Vec<u8> = percent_decode_str(encoded).collect();
    if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.iter().map(|&b| b as char).collect())
    } else {
        String::from_utf8(bytes).ok()
    }
}

pub fn file_name_from_url(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    let last = u.path_segments()?.rfind(|s| !s.is_empty())?;
    let decoded = percent_decode_str(last).decode_utf8_lossy().into_owned();
    Some(decoded).filter(|s| !s.is_empty())
}

/// The name a download is saved under: the caller's choice, else the server's `Content-Disposition`, else the
/// last part of the link (as typed, then after redirects), else `download`. Always safe to create.
pub fn derive_file_name(preferred: Option<&str>, content_disposition: Option<&str>, url: &str, final_url: &str) -> String {
    sanitize_file_name(
        &preferred
            .map(str::to_owned)
            .or_else(|| content_disposition.and_then(parse_content_disposition))
            .or_else(|| file_name_from_url(url))
            .or_else(|| file_name_from_url(final_url))
            .unwrap_or_else(|| "download".into()),
    )
}

/// Makes a name safe to create on Linux, macOS and Windows: no separators or control characters,
/// no reserved device names, no trailing dots/spaces, at most 200 bytes (extension kept).
pub fn sanitize_file_name(name: &str) -> String {
    // A suggested name may carry directories ("sub/file.zip"): keep the base name only.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut s: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"|?*"#.contains(c) { '_' } else { c })
        .collect();
    s = s.trim().trim_end_matches(['.', ' ']).trim_start_matches('.').to_string();
    let stem_upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem_upper.as_str()) {
        s.insert(0, '_');
    }
    if s.len() > 200 {
        let (stem, ext) = match s.rfind('.') {
            Some(i) if s.len() - i <= 16 => (s[..i].to_string(), s[i..].to_string()),
            _ => (s.clone(), String::new()),
        };
        let mut cut = 200 - ext.len();
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        s = format!("{}{}", &stem[..cut], ext);
    }
    if s.is_empty() { "download".into() } else { s }
}

/// Positional write that does not move any shared cursor, so segments can share one `File`.
#[cfg(unix)]
pub fn write_all_at(f: &File, buf: &[u8], pos: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, pos)
}

#[cfg(windows)]
pub fn write_all_at(f: &File, mut buf: &[u8], mut pos: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, pos)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        pos += n as u64;
    }
    Ok(())
}

pub fn part_path(dest_dir: &Path, id: u64) -> PathBuf {
    dest_dir.join(format!(".{id:x}.admpart"))
}

/// Moves `part` to `dir/name`, picking `name (1).ext`, `name (2).ext`, ... if taken. The name is
/// claimed with `create_new` first so two downloads finishing together never pick the same one.
pub fn publish(part: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 0..10_000u32 {
        let candidate = if n == 0 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let target = dir.join(&candidate);
        match OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(_) => {
                if let Err(e) = std::fs::rename(part, &target) {
                    let _ = std::fs::remove_file(&target);
                    return Err(e);
                }
                return Ok(target);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free file name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-999/1000"), Some((0, Some(999), Some(1000))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, Some(9), None)));
        assert_eq!(parse_content_range("bytes */1000"), None);
    }

    #[test]
    fn disposition() {
        assert_eq!(parse_content_disposition(r#"attachment; filename="a b.zip""#).as_deref(), Some("a b.zip"));
        assert_eq!(parse_content_disposition("attachment; filename=plain.txt").as_deref(), Some("plain.txt"));
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="fallback.txt"; filename*=UTF-8''na%C3%AFve.txt"#).as_deref(),
            Some("naïve.txt")
        );
        assert_eq!(parse_content_disposition(r#"attachment; filename="a;b.txt""#).as_deref(), Some("a;b.txt"));
        assert_eq!(parse_content_disposition("inline"), None);
    }

    #[test]
    fn derived_names_prefer_the_callers_then_the_servers() {
        let cd = Some(r#"attachment; filename="server.zip""#);
        assert_eq!(derive_file_name(Some("mine.zip"), cd, "https://x/a.zip", "https://y/b.zip"), "mine.zip");
        assert_eq!(derive_file_name(None, cd, "https://x/a.zip", "https://y/b.zip"), "server.zip");
        assert_eq!(derive_file_name(None, None, "https://x/a.zip?t=1", "https://y/b.zip"), "a.zip");
        assert_eq!(derive_file_name(None, None, "https://x/", "https://y/b.zip"), "b.zip");
        assert_eq!(derive_file_name(None, None, "https://x/", "https://y/"), "download");
        assert_eq!(derive_file_name(Some("../../etc/passwd"), None, "https://x/", "https://x/"), "passwd");
    }

    #[test]
    fn names() {
        assert_eq!(sanitize_file_name("sub/dir/file.zip"), "file.zip");
        assert_eq!(sanitize_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("..."), "download");
        assert_eq!(file_name_from_url("https://x.io/a/b%20c.iso?token=1").as_deref(), Some("b c.iso"));
        assert!(sanitize_file_name(&format!("{}.mp4", "x".repeat(400))).len() <= 200);
    }
}
