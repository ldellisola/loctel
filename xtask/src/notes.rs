//! The text of a GitHub release: what changed, what to check on a Mac, and
//! how to install an unsigned app.
//!
//! The checklist is `docs/milestones/mac-checklist.md`, carried over word for
//! word (design 10.4): the person who tries the release reads exactly what
//! the repository says to try.

/// How to open an app that is only ad-hoc signed (design 10.3). Gatekeeper
/// asks again on the first launch of every version, because each download is
/// a new quarantined file.
const INSTALLING: &str = "\
## Installing

1. Download the `.dmg` below, open it, and drag `loctel` onto the
   `Applications` folder next to it.
2. Open `loctel` from Applications. macOS says it cannot verify the app: loctel
   is signed ad hoc, without an Apple Developer certificate. Close that
   message.
3. Open System Settings, then Privacy & Security. Scroll to the Security
   section, find the message that `loctel` was blocked, click **Open Anyway**
   and confirm with your password or Touch ID. Open `loctel` again.
4. Do this again on the first launch of each new version.

To skip the clicks, remove the quarantine mark in a terminal before the first
launch:

```bash
xattr -dr com.apple.quarantine /Applications/loctel.app
```

loctel needs macOS 26 on Apple Silicon. Look for its icon in the menu bar: it
has a Dock icon only while its window is open (menu bar icon, then **Open
loctel**).
";

/// The notes of a release.
///
/// - `subjects`: the first lines of the commit messages since the previous
///   release, newest first. Blank ones are skipped. With squash merges each is
///   a pull request title, so the list is the PRs merged (design 10.3).
/// - `checklist_md`: the "Check on your Mac" list, inserted unchanged.
///
/// The result is a "Changes" list, then "Check on your Mac", then
/// "Installing", ending with one newline.
pub fn release_notes(subjects: &[String], checklist_md: &str) -> String {
    let mut notes = String::from("## Changes\n\n");
    let mut any = false;
    for subject in subjects.iter().map(|subject| subject.trim()) {
        if !subject.is_empty() {
            notes.push_str("- ");
            notes.push_str(subject);
            notes.push('\n');
            any = true;
        }
    }
    if !any {
        notes.push_str("No changes recorded.\n");
    }

    notes.push_str("\n## Check on your Mac\n\n");
    notes.push_str(checklist_md);
    if !checklist_md.ends_with('\n') {
        notes.push('\n');
    }

    notes.push('\n');
    notes.push_str(INSTALLING);
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subjects(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    const CHECKLIST: &str = "1. Install it.\n2. Look at the menu bar.\n";

    #[test]
    fn notes_contain_checklist_and_install_steps() {
        let notes = release_notes(&subjects(&["feat(store): logs (#3)"]), CHECKLIST);

        assert!(notes.contains("## Changes"));
        assert!(notes.contains("## Check on your Mac"));
        assert!(notes.contains("## Installing"));
        // The checklist is carried over exactly as written.
        assert!(notes.contains(CHECKLIST));
        // The two ways past Gatekeeper.
        assert!(notes.contains("Open Anyway"));
        assert!(notes.contains("xattr -dr com.apple.quarantine /Applications/loctel.app"));
    }

    #[test]
    fn sections_come_in_the_order_changes_checklist_installing() {
        let notes = release_notes(&subjects(&["fix: a"]), CHECKLIST);

        let changes = notes.find("## Changes").unwrap();
        let checklist = notes.find("## Check on your Mac").unwrap();
        let installing = notes.find("## Installing").unwrap();
        assert!(changes < checklist, "{notes}");
        assert!(checklist < installing, "{notes}");
        assert!(changes < notes.find("fix: a").unwrap());
        assert!(notes.find("fix: a").unwrap() < checklist);
        assert!(checklist < notes.find("Install it.").unwrap());
        assert!(notes.find("2. Look at the menu bar.").unwrap() < installing);
    }

    #[test]
    fn every_subject_is_a_bullet_in_the_order_given() {
        let notes = release_notes(
            &subjects(&[
                "feat(mcp): tools (#12)",
                "",
                "  fix(ui): font \n",
                "docs: x",
            ]),
            CHECKLIST,
        );

        let wanted = "- feat(mcp): tools (#12)\n- fix(ui): font\n- docs: x\n";
        assert!(notes.contains(wanted), "{notes}");
    }

    #[test]
    fn no_subjects_still_gives_a_changes_section() {
        let notes = release_notes(&[], CHECKLIST);

        assert!(
            notes.contains("## Changes\n\nNo changes recorded."),
            "{notes}"
        );
        assert!(notes.contains(CHECKLIST));
    }

    #[test]
    fn a_checklist_without_a_final_newline_does_not_run_into_the_next_heading() {
        let notes = release_notes(&[], "1. Only item.");

        assert!(notes.contains("1. Only item.\n\n## Installing"), "{notes}");
    }

    #[test]
    fn the_notes_end_with_one_newline() {
        let notes = release_notes(&subjects(&["feat: a"]), CHECKLIST);

        assert!(notes.ends_with('\n'));
        assert!(!notes.ends_with("\n\n"));
    }
}
