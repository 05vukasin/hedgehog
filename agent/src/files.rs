//! File access bounds and the `list_files` tool.
//!
//! Every path a tool receives is resolved against the root folder, canonicalized (symlinks and
//! `..` resolved) and must then lie inside the root or be one of the explicitly allowed paths
//! (files the user referenced, or folders).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::text::cut;

/// Spreadsheet extensions `list_files` reports and `open_file` accepts.
pub const EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "ods", "csv", "tsv"];
const MAX_LINES: usize = 200;
const MAX_DEPTH: usize = 6;
const WALK_BUDGET: usize = 50_000;

#[derive(Debug, Clone)]
pub struct Bounds {
    root: PathBuf,
    /// Canonical allowed paths and whether each is a directory.
    allow: Vec<(PathBuf, bool)>,
}

impl Bounds {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Bounds> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotADirectory, format!("{} is not a folder", root.display())));
        }
        Ok(Bounds { root, allow: Vec::new() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Allows access to one more file or folder (outside the root). Paths that do not exist are
    /// rejected.
    pub fn allow(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let p = path.as_ref();
        let c = p.canonicalize().map_err(|e| format!("{}: {e}", p.display()))?;
        let dir = c.is_dir();
        if !self.allow.iter().any(|(a, _)| *a == c) {
            self.allow.push((c, dir));
        }
        Ok(())
    }

    fn permitted(&self, canon: &Path) -> bool {
        canon.starts_with(&self.root) || self.allow.iter().any(|(a, dir)| canon == a || (*dir && canon.starts_with(a)))
    }

    /// Resolves a path a tool received (relative to the root; `@` prefix and quotes stripped).
    pub fn resolve(&self, raw: &str, want_dir: bool) -> Result<PathBuf, String> {
        let s = clean(raw);
        if s.is_empty() {
            return if want_dir { Ok(self.root.clone()) } else { Err("path is empty".into()) };
        }
        let p = Path::new(&s);
        let joined = if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) };
        let canon = joined.canonicalize().map_err(|_| {
            format!("no such {}: {s} (paths are relative to the working folder; list_files shows what is there)", if want_dir { "folder" } else { "file" })
        })?;
        if !self.permitted(&canon) {
            return Err(format!("access denied: {s} is outside the working folder"));
        }
        if want_dir && !canon.is_dir() {
            return Err(format!("{s} is not a folder"));
        }
        if !want_dir && !canon.is_file() {
            return Err(format!("{s} is not a file"));
        }
        Ok(canon)
    }

    /// Path as shown to the model: relative to the root when inside it, absolute otherwise.
    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.root) {
            Ok(r) if r.as_os_str().is_empty() => ".".into(),
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => p.display().to_string(),
        }
    }

    /// The `list_files` tool.
    pub fn list_files(&self, folder: Option<&str>, pattern: Option<&str>) -> Result<String, String> {
        let dir = self.resolve(folder.unwrap_or("."), true)?;
        let pattern = pattern.map(str::trim).filter(|p| !p.is_empty());
        let mut found: Vec<(PathBuf, u64, Option<SystemTime>)> = Vec::new();
        let mut budget = WALK_BUDGET;
        let mut truncated = false;
        let mut stack = vec![(dir.clone(), 0usize)];
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            let mut subdirs = Vec::new();
            for e in entries {
                if budget == 0 {
                    truncated = true;
                    break;
                }
                budget -= 1;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name.starts_with("~$") {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                let path = e.path();
                if ft.is_symlink() {
                    // Follow only links that stay within bounds; never into directories.
                    match path.canonicalize() {
                        Ok(c) if c.is_file() && self.permitted(&c) => {}
                        _ => continue,
                    }
                } else if ft.is_dir() {
                    if depth < MAX_DEPTH {
                        subdirs.push((path, depth + 1));
                    }
                    continue;
                }
                let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
                if !EXTENSIONS.contains(&ext.as_str()) {
                    continue;
                }
                let rel = path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                if let Some(p) = pattern
                    && !matches(p, &rel)
                {
                    continue;
                }
                let meta = std::fs::metadata(&path).ok();
                found.push((path, meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok())));
            }
            stack.extend(subdirs.into_iter().rev());
        }
        let where_ = self.display(&dir);
        if found.is_empty() {
            return Ok(match pattern {
                Some(p) => format!(
                    "no spreadsheet file under {where_} matches {p:?}; file names are often in the data's language — try other words, alternatives with | (preis|price), or no pattern"
                ),
                None => format!("no spreadsheet files under {where_}"),
            });
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        let mut lines = vec![format!(
            "{} spreadsheet file{} under {where_}{} (path · size · modified):",
            found.len(),
            if found.len() == 1 { "" } else { "s" },
            pattern.map(|p| format!(" matching {p:?}")).unwrap_or_default()
        )];
        for (p, size, modified) in found.iter().take(MAX_LINES) {
            lines.push(format!("{} · {} · {}", self.display(p), human_size(*size), modified.map(ymd).unwrap_or_default()));
        }
        if found.len() > MAX_LINES {
            lines.push(format!(
                "… cut: {} more files not listed; ask for list_files with a subfolder or a pattern",
                found.len() - MAX_LINES
            ));
        }
        if truncated {
            lines.push(format!("(stopped after {WALK_BUDGET} entries; list a subfolder for the rest)"));
        }
        Ok(cut(lines.join("\n"), crate::text::MAX_CHARS, |n| format!("{n} more files"), "list_files with a subfolder or a pattern"))
    }
}

fn clean(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix('@').unwrap_or(s);
    let s = s.trim_matches(|c| c == '"' || c == '\'' || c == '`');
    s.trim().to_string()
}

/// `pattern` against a relative path, case-insensitive: a glob when it has `*` or `?` (on the file
/// name, or on the whole path when the pattern has a `/`), else fragments: `a b` = all of,
/// `a|b` = any of.
pub fn matches(pattern: &str, rel: &str) -> bool {
    let p = pattern.to_lowercase();
    let rel = rel.to_lowercase();
    if p.contains(['*', '?']) {
        let target = if p.contains('/') { rel.as_str() } else { rel.rsplit('/').next().unwrap_or(&rel) };
        return glob(p.as_bytes(), target.as_bytes());
    }
    p.split('|').any(|alt| {
        let words: Vec<&str> = alt.split_whitespace().collect();
        !words.is_empty() && words.iter().all(|w| rel.contains(w))
    })
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// `YYYY-MM-DD` (UTC) of a timestamp.
fn ymd(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86_400);
    // civil-from-days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hh_agent_files_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("root/sub")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        std::fs::write(d.join("root/a.xlsx"), b"x").unwrap();
        std::fs::write(d.join("root/sub/Preise 2022.csv"), b"x").unwrap();
        std::fs::write(d.join("root/notes.txt"), b"x").unwrap();
        std::fs::write(d.join("outside/secret.csv"), b"x").unwrap();
        d
    }

    #[test]
    fn path_bounds() {
        let d = tmp("bounds");
        let mut b = Bounds::new(d.join("root")).unwrap();
        assert!(b.resolve("a.xlsx", false).is_ok());
        assert!(b.resolve("@sub/Preise 2022.csv", false).is_ok());
        assert!(b.resolve("\"sub/../a.xlsx\"", false).is_ok());
        let e = b.resolve("../outside/secret.csv", false).unwrap_err();
        assert!(e.contains("outside the working folder"), "{e}");
        let abs = d.join("outside/secret.csv");
        assert!(b.resolve(abs.to_str().unwrap(), false).is_err());
        assert!(b.resolve("/etc/passwd", false).is_err());
        assert!(b.resolve("missing.xlsx", false).unwrap_err().contains("no such file"));
        assert!(b.resolve("..", true).is_err());
        // An allowed file outside the root opens; its siblings do not.
        b.allow(&abs).unwrap();
        assert!(b.resolve(abs.to_str().unwrap(), false).is_ok());
        assert!(b.resolve(d.join("outside").to_str().unwrap(), true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("outside"), d.join("root/link")).unwrap();
            assert!(b.resolve("link/secret.csv", false).is_ok(), "allowed file through a link");
            std::fs::write(d.join("outside/other.csv"), b"x").unwrap();
            assert!(b.resolve("link/other.csv", false).unwrap_err().contains("outside"));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn listing_and_patterns() {
        let d = tmp("list");
        let b = Bounds::new(d.join("root")).unwrap();
        let all = b.list_files(None, None).unwrap();
        assert!(all.starts_with("2 spreadsheet files under ."), "{all}");
        assert!(all.contains("sub/Preise 2022.csv"));
        assert!(!all.contains("notes.txt"));
        assert!(b.list_files(None, Some("preis|price")).unwrap().contains("Preise"));
        assert!(b.list_files(None, Some("*.xlsx")).unwrap().contains("a.xlsx"));
        assert!(b.list_files(None, Some("preise 2023")).unwrap().starts_with("no spreadsheet file"));
        assert!(b.list_files(Some(".."), None).is_err());
        assert!(matches("sub/*.csv", "sub/preise 2022.csv"));
        assert!(!matches("*.xlsx", "a.xls"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dates() {
        assert_eq!(ymd(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)), "2023-11-14");
    }
}
