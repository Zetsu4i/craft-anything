//! Notes the person adds while the builder works, handed over at the builder's next step instead
//! of after its whole turn. The run writes each one to `inbox/notes/<id>.json`; a coding agent
//! that runs a command after every tool call (Claude Code's PostToolUse hook, running
//! `craft-anything-cli builder-notes <dir>`) claims what's there by renaming it to
//! `<id>.read.json` and gives the text to the model; the run sees the rename and shows the note as
//! read. The rename is the one moment a note changes hands, so "take back" works until then.
//! Notes nobody claimed still go in after the turn, as before. A note's file also keeps the
//! person's own words ([`Left`]), so a note still waiting when the app closes isn't lost: the
//! next run takes it as its own waiting note ([`take_left`]).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a project's notes wait for the builder.
pub fn notes_dir(inbox: &Path) -> PathBuf {
    inbox.join("notes")
}

fn pending(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn read(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.read.json"))
}

/// A note as the person sent it: what a later run sends if this one ends before handing it over.
/// `text` is without the Just-ask answers the hand-over text may add, so a run in the other mode
/// adds only the ones it may show.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Left {
    pub text: String,
    pub shown: String,
    #[serde(default)]
    pub pictures: Vec<PathBuf>,
}

/// Leave a note for the builder's next step: `text` is what the agent's hook hands over.
pub fn hand_over(dir: &Path, id: u64, text: &str, left: Option<&Left>) -> Result<(), craft_anything_evidence::ToolError> {
    std::fs::create_dir_all(dir).map_err(|e| craft_anything_evidence::ToolError::io(dir.display(), e))?;
    let bytes = serde_json::to_vec(&serde_json::json!({"id": id, "text": text, "left": left}))
        .map_err(|e| craft_anything_evidence::ToolError::internal(e.to_string()))?;
    craft_anything_engine::project::write_atomic(&pending(dir, id), &bytes)
}

/// The notes an earlier run left waiting (the app closed before the builder took them, #1620),
/// oldest first. A file without the person's words (from before they were kept) can't be sent
/// safely and is left out, as is one that isn't a note; read notes were handed over already. A
/// folder or file that can't be read is an error, not "no notes".
fn left(dir: &Path) -> std::io::Result<Vec<Left>> {
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        r => r?,
    };
    let mut ids = Vec::new();
    for entry in entries {
        if let Some(id) = entry?.file_name().to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u64>().ok()) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    let mut notes = Vec::new();
    for id in ids {
        let bytes = std::fs::read(pending(dir, id))?;
        let note = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|file| serde_json::from_value::<Left>(file.get("left")?.clone()).ok());
        notes.extend(note.filter(|l| !l.text.trim().is_empty()));
    }
    Ok(notes)
}

/// Start a run's notes: an empty folder, and the notes an earlier run left waiting, for this run
/// to wait with until the builder takes them (`wanted` is false for a run that brings its own
/// message, which already carries them). Ids restart with the process, so the old files can't
/// stay. When they can't be read, the folder is moved aside, not deleted.
pub fn take_left(dir: &Path, wanted: bool) -> Vec<Left> {
    match left(dir) {
        Ok(notes) => {
            clear(dir);
            if wanted { notes } else { Vec::new() }
        }
        Err(e) => {
            let aside = dir
                .with_file_name(format!("notes-unread-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()));
            match std::fs::rename(dir, &aside) {
                Ok(()) => log::warn!("Couldn't read the notes waiting in {} ({e}); they're kept in {}", dir.display(), aside.display()),
                Err(moved) => log::warn!("Couldn't read the notes waiting in {} ({e}), or move them aside ({moved})", dir.display()),
            }
            Vec::new()
        }
    }
}

/// Take a note back before the builder read it: true when it was still waiting.
pub fn take_back(dir: &Path, id: u64) -> bool {
    std::fs::remove_file(pending(dir, id)).is_ok()
}

/// The builder has read the note (it was claimed).
pub fn was_read(dir: &Path, id: u64) -> bool {
    read(dir, id).is_file()
}

/// Done with a note: its files go.
pub fn forget(dir: &Path, id: u64) {
    let _ = std::fs::remove_file(pending(dir, id));
    let _ = std::fs::remove_file(read(dir, id));
}

/// Start a run with no notes left over from an earlier one (ids restart with the process).
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// For the agent's hook: claim every waiting note, oldest first, and return their texts. A note
/// another claimer (or "take back") got first is skipped.
pub fn claim(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut ids: Vec<u64> =
        entries.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u64>().ok())).collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| {
            let text = std::fs::read(pending(dir, id)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())?;
            std::fs::rename(pending(dir, id), read(dir, id)).ok()?;
            text.get("text").and_then(serde_json::Value::as_str).map(String::from)
        })
        .collect()
}

/// What the hook prints for the agent: the notes as context for its next step, or nothing.
pub fn hook_output(texts: &[String]) -> Option<String> {
    if texts.is_empty() {
        return None;
    }
    let list = texts.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n");
    // Stated as facts: the agent reads it as context, not as an order from the tool.
    let context = format!(
        "The person added this while you were working:\n{list}\nIt applies to the work from here on, and your reply can say how you took it into account."
    );
    Some(serde_json::json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": context}}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_changes_hands_once_and_can_be_taken_back_until_then() {
        let d = tempfile::tempdir().unwrap();
        let dir = notes_dir(d.path());
        assert!(claim(&dir).is_empty(), "no folder, nothing to claim");
        hand_over(&dir, 2, "make the header blue", None).unwrap();
        hand_over(&dir, 10, "add a contact form", None).unwrap();
        hand_over(&dir, 3, "never mind the footer", None).unwrap();
        assert!(take_back(&dir, 3), "still waiting: it can be taken back");
        assert_eq!(claim(&dir), ["make the header blue", "add a contact form"], "oldest first, by number");
        assert!(was_read(&dir, 2) && was_read(&dir, 10) && !was_read(&dir, 3));
        assert!(!take_back(&dir, 2), "read notes can't be taken back");
        assert!(claim(&dir).is_empty(), "each note is handed over once");
        forget(&dir, 2);
        assert!(!was_read(&dir, 2));
        clear(&dir);
        assert!(!dir.exists());
    }

    #[test]
    fn notes_nobody_took_are_left_for_the_next_run() {
        let d = tempfile::tempdir().unwrap();
        let dir = notes_dir(d.path());
        assert!(left(&dir).unwrap().is_empty(), "no folder, nothing left");
        let note = |text: &str| Left { text: text.into(), shown: format!("{text} (shown)"), pictures: vec![PathBuf::from("pictures/a.png")] };
        hand_over(&dir, 10, "add a form, with what was asked in Just ask", Some(&note("add a form"))).unwrap();
        hand_over(&dir, 2, "make it blue", Some(&note("make it blue"))).unwrap();
        hand_over(&dir, 3, "taken by the hook", Some(&note("taken by the hook"))).unwrap();
        hand_over(&dir, 4, "from an older version", None).unwrap();
        hand_over(&dir, 5, " ", Some(&note(" "))).unwrap();
        std::fs::write(dir.join("6.json"), b"{not json").unwrap();
        std::fs::rename(dir.join("3.json"), dir.join("3.read.json")).unwrap();
        assert_eq!(left(&dir).unwrap(), [note("make it blue"), note("add a form")], "oldest first, the person's words without the answers");
        assert_eq!(claim(&dir)[0], "make it blue", "the hook still reads the hand-over text");
    }

    #[test]
    fn a_run_takes_the_left_notes_and_starts_with_an_empty_folder() {
        let d = tempfile::tempdir().unwrap();
        let dir = notes_dir(d.path());
        let note = Left { text: "make it blue".into(), shown: "make it blue".into(), pictures: Vec::new() };
        assert!(take_left(&dir, true).is_empty(), "no folder, nothing left");
        hand_over(&dir, 1, "make it blue", Some(&note)).unwrap();
        assert!(take_left(&dir, false).is_empty() && !dir.exists(), "a run with its own message clears them");
        hand_over(&dir, 1, "make it blue", Some(&note)).unwrap();
        assert_eq!(take_left(&dir, true), std::slice::from_ref(&note));
        assert!(!dir.exists(), "their old ids can't clash with this run's");

        // One that can't be read: nothing is deleted, the folder is kept aside.
        hand_over(&dir, 1, "make it blue", Some(&note)).unwrap();
        std::fs::create_dir_all(dir.join("2.json")).unwrap();
        assert!(left(&dir).is_err());
        assert!(take_left(&dir, true).is_empty() && !dir.exists());
        let aside: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert!(aside[0].file_name().unwrap().to_string_lossy().starts_with("notes-unread-") && aside[0].join("1.json").is_file());
    }

    #[test]
    fn the_hook_hands_the_notes_to_the_model_as_context() {
        assert!(hook_output(&[]).is_none(), "nothing printed when nothing waits");
        let out: serde_json::Value = serde_json::from_str(&hook_output(&["make it blue".into()]).unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let context = out["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(context.contains("- make it blue") && context.contains("applies to the work from here on"), "{context}");
    }
}
