//! Minimal persistence for frecency until the SQLite store lands (docs/03 §11).

use std::path::PathBuf;

use serde::Serialize;
use swirvo_wheel::Frecency;

/// `%LOCALAPPDATA%\Swirvo` on Windows, `$XDG_DATA_HOME/swirvo` (or ~/.local/share) elsewhere.
/// `SWIRVO_DATA_DIR` overrides both (portable installs, tests).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SWIRVO_DATA_DIR") {
        return Some(PathBuf::from(p));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Swirvo"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|p| p.join("swirvo"))
}

#[derive(Serialize, serde::Deserialize)]
struct Row {
    ctx: String,
    target: String,
    score: f64,
    at: u64,
}

fn path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("frecency.json"))
}

pub fn load_frecency() -> Frecency {
    let mut f = Frecency::default();
    if let Some(rows) = path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Vec<Row>>(&b).ok())
    {
        for r in rows {
            f.restore(&r.ctx, &r.target, r.score, r.at);
        }
    }
    f
}

/// Best effort: a failure to save usage stats must never fail a conversion.
pub fn save_frecency(f: &Frecency) {
    let Some(p) = path() else { return };
    let rows: Vec<Row> = f
        .entries()
        .map(|(c, t, s, at)| Row {
            ctx: c.into(),
            target: t.into(),
            score: s,
            at,
        })
        .collect();
    if let (Some(dir), Ok(json)) = (p.parent(), serde_json::to_vec(&rows)) {
        let _ = std::fs::create_dir_all(dir);
        // A unique temp name per writer, then an atomic rename: concurrent saves (the agent
        // saves off-thread) can't interleave, and readers never see a half-written file.
        // Replacing is right here: this is our own state file, never a user file.
        if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(dir) {
            use std::io::Write;
            if tmp.write_all(&json).is_ok() {
                let _ = tmp.persist(&p);
            }
        }
    }
}
