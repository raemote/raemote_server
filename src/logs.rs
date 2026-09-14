//! Reading the daemon's rotating log files for the `raemote logs` command.

use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Prefix/suffix used by the daemon's rotating log files (`raemoted.<date>.log`).
pub const LOG_PREFIX: &str = "raemoted.";
/// Suffix of daemon log file names.
pub const LOG_SUFFIX: &str = ".log";

/// How much of the end of the file to read when tailing.
const TAIL_WINDOW: u64 = 256 * 1024;

/// Return the most recent daemon log file in `dir`, if any.
///
/// Log file names embed a `YYYY-MM-DD` date, so the lexicographically greatest
/// matching name is the newest.
pub fn latest_log_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(OsString, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !(name_str.starts_with(LOG_PREFIX) && name_str.ends_with(LOG_SUFFIX)) {
            continue;
        }
        let replace = match &best {
            Some((current, _)) => name > *current,
            None => true,
        };
        if replace {
            best = Some((name, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

/// Return up to the last `lines` lines of `path`.
///
/// Reads only the tail of the file (bounded), tolerates a partial first line
/// when the file is larger than the read window, and never fails on non-UTF-8
/// bytes (they are replaced).
pub fn tail_lines(path: &Path, lines: usize) -> Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let text = String::from_utf8_lossy(&buf);
    let mut all: Vec<&str> = text.lines().collect();
    // If we began reading mid-file, the first line is likely partial: drop it.
    if start > 0 && !all.is_empty() {
        all.remove(0);
    }
    let from = all.len().saturating_sub(lines);
    Ok(all[from..].iter().map(|line| strip_ansi(line)).collect())
}

/// Remove ANSI SGR escape sequences (e.g. `ESC[3m`).
///
/// Some dependencies emit colored span fields even when the log layer has
/// `ansi(false)`, so we sanitize on read to keep `raemote logs` output clean.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        // CSI sequence: ESC '[' ... final byte in '@'..='~'.
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raemoted.2026-01-01.log");
        let content: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, content).unwrap();

        assert_eq!(
            tail_lines(&path, 3).unwrap(),
            vec!["line 98", "line 99", "line 100"]
        );
    }

    #[test]
    fn tail_handles_fewer_lines_than_requested() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raemoted.2026-01-01.log");
        std::fs::write(&path, "only\n").unwrap();
        assert_eq!(tail_lines(&path, 10).unwrap(), vec!["only"]);
    }

    #[test]
    fn latest_log_file_picks_newest_and_ignores_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("raemoted.2026-01-01.log"), "a").unwrap();
        std::fs::write(dir.path().join("raemoted.2026-02-01.log"), "b").unwrap();
        std::fs::write(dir.path().join("other.log"), "c").unwrap();

        let latest = latest_log_file(dir.path()).unwrap();
        assert_eq!(latest.file_name().unwrap(), "raemoted.2026-02-01.log");
    }

    #[test]
    fn latest_log_file_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(latest_log_file(dir.path()).is_none());
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        let colored = "a\u{1b}[3mid\u{1b}[0m\u{1b}[2m=\u{1b}[0mb";
        assert_eq!(strip_ansi(colored), "aid=b");
        assert_eq!(strip_ansi("plain text"), "plain text");
    }
}
