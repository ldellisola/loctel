//! The live data of the Discard rules screen (design 9.2 and 7, M4 plan Task
//! 5): the rules as the backend has them, with what each has dropped, and the
//! rule being edited, if one is.
//!
//! [`RulesModel`] is an entity the rules views observe, like
//! [`HomeModel`](super::HomeModel) for the home screen. It holds:
//!
//! - **The list.** The rules (`UiBackend::rules`), their cards
//!   ([`model::rules::rows`](crate::model::rules::rows)), the share of the
//!   incoming data the rules dropped and the error of the rules file, if it has
//!   one. It is read again every [`RULES_REFRESH`] (1 s) while the Rules screen
//!   shows, which is how a reload of an edited file and the counters get there,
//!   and at once after anything this model does. These are in-memory reads (the
//!   engine's rules, no store), so they run on the foreground like the shell's
//!   status.
//! - **The editor session** ([`RuleEditor`]): the draft in the fields, the
//!   rules as the editor saw them when it opened, the preview of the draft, and
//!   whether the rules changed behind its back. `Save` builds the full list
//!   from that snapshot with the draft in the place of its rule (or at the end,
//!   for a new rule) and hands it to `UiBackend::set_rules`.
//!
//! **A rule edited in the file while the editor has a draft of it** (plan
//! Review Focus 2). The editor remembers `UiBackend::rules_generation` from when
//! it opened. When the number has moved and this model did not move it, the
//! rules changed behind the editor's back: [`RuleEditor::external_change`]
//! turns on (the view shows the banner), the draft is left exactly as it is, and
//! `Save` does nothing until the user decides. [`keep_draft`](RulesModel::keep_draft)
//! turns the banner off, and then `Save` writes the snapshot with the draft over
//! the file, which is the overwriting the banner warned about;
//! [`discard_draft`](RulesModel::discard_draft) closes the editor. A change this
//! model made itself (a switch of a card, a save) is not one: it takes the
//! generation it caused, and re-takes the snapshot, so the list the editor saves
//! over is never older than the user's own last action.
//!
//! **The preview** ("Tried on the last 5 minutes") is `loctel_rules::preview` of
//! the draft over the last [`PREVIEW_WINDOW`](loctel_rules::PREVIEW_WINDOW) of
//! the store. It is a pass over the stored data, so it runs on GPUI's background
//! executor on a copy of the draft and a clone of the store (the same call as
//! `UiBackend::preview_rule`, which cannot leave the foreground: decision
//! 0038). A change of the text waits [`PREVIEW_DEBOUNCE`] (300 ms) after the
//! last one; opening a rule, and a click on the language or the scope, run at
//! once. A query that does not parse says so at once, with no read; a blank one
//! has no preview. A result that a newer edit replaced is thrown away when it
//! arrives.
//!
//! The model reads and writes only while it is *active* (the shell sets it as the
//! Rules screen comes and goes): a screen that is not showing costs nothing.
//! `docs/areas/ui.md` ("The Discard rules screen") has the rules, and decision
//! 0038 the choices.

use std::rc::Rc;
use std::time::Duration;

use gpui::Context;
use loctel_query::QueryError;
use loctel_rules::{
    DropScope, PREVIEW_WINDOW, Preview, RuleId, RuleLanguage, RuleSpec, RuleStatus,
};
use loctel_store::Store;

use super::backend::UiBackend;
use crate::model::rules::{self, Draft, PREVIEW_INVALID, RuleRow};

/// How often the rules list is read again while the Rules screen shows.
pub const RULES_REFRESH: Duration = Duration::from_secs(1);

/// How long the editor waits after the last change of a text before it runs the
/// preview.
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(300);

/// What the editor's preview box has to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewState {
    /// Nothing to show yet: the query is blank, or its preview has not come
    /// back. A result that was there stays until the next one replaces it.
    Pending,
    /// The query does not parse.
    Invalid,
    /// What the draft would have dropped of the last five minutes, and which
    /// language it was tried as (the sentence differs).
    Ready {
        language: RuleLanguage,
        preview: Preview,
    },
}

/// A rule open in the editor.
#[derive(Clone, Debug)]
pub struct RuleEditor {
    /// Counts the editors opened: the view puts the draft's text in its inputs
    /// when this changes, and only then.
    session: u64,
    /// The saved rule being edited, `None` for a new one.
    target: Option<RuleId>,
    draft: Draft,
    /// The rules as the editor saw them: what `Save` and `Delete` write over.
    base: Vec<(RuleId, RuleSpec)>,
    /// The rules generation the editor has accounted for.
    generation: u64,
    external_change: bool,
    confirming_delete: bool,
    /// Why the last save or delete did not go through.
    error: Option<String>,
    preview: PreviewState,
    /// Counts the previews asked for: only the latest one's result is shown.
    preview_generation: u64,
}

impl RuleEditor {
    /// Which editor this is: a different number is a different opening, even for
    /// the same rule.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// The saved rule being edited, `None` for a rule that is not in the list
    /// yet (`New rule`, or the suggestion of `Discard lines like this…`).
    pub fn target(&self) -> Option<RuleId> {
        self.target
    }

    /// Whether the rule is new: not saved yet, so there is nothing to delete.
    pub fn is_new(&self) -> bool {
        self.target.is_none()
    }

    /// What the fields hold.
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// The rules changed behind the editor's back and the user has not decided
    /// yet: the banner shows and `Save` does nothing.
    pub fn external_change(&self) -> bool {
        self.external_change
    }

    /// `Delete` was pressed and the editor is asking.
    pub fn confirming_delete(&self) -> bool {
        self.confirming_delete
    }

    /// Why the last save or delete failed (the file could not be written).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The state of the preview box.
    pub fn preview(&self) -> &PreviewState {
        &self.preview
    }

    /// The preview box's sentence: `None` while there is nothing to say,
    /// `Fix the query to see what it would drop.` for a query that does not
    /// parse, else what it would have dropped.
    pub fn preview_text(&self) -> Option<String> {
        match &self.preview {
            PreviewState::Pending => None,
            PreviewState::Invalid => Some(PREVIEW_INVALID.to_owned()),
            PreviewState::Ready { language, preview } => {
                Some(rules::preview_text(*language, preview))
            }
        }
    }

    /// Whether `Save` is enabled: the draft has a name, the rules did not change
    /// behind the editor's back, and the editor is not asking about a delete.
    pub fn can_save(&self) -> bool {
        self.draft.can_save() && !self.external_change && !self.confirming_delete
    }

    /// Where the rule being edited stands in the snapshot.
    fn position(&self) -> Option<usize> {
        let target = self.target?;
        self.base.iter().position(|(id, _)| *id == target)
    }
}

/// The Discard rules screen's data and the editor session. Make it with
/// `cx.new(|cx| RulesModel::new(backend, cx))` and observe it from the views.
pub struct RulesModel {
    backend: Rc<dyn UiBackend>,
    statuses: Vec<RuleStatus>,
    rows: Vec<RuleRow>,
    dropped_fraction: f64,
    file_error: Option<String>,
    /// Why the last switch of a card did not go through, shown over the list.
    action_error: Option<String>,
    /// The Rules screen is the one showing (the shell says so).
    active: bool,
    editor: Option<RuleEditor>,
    /// How many editors have been opened.
    sessions: u64,
}

impl RulesModel {
    /// The model with the rules as the backend has them now, and no editor. It
    /// reads again every [`RULES_REFRESH`] once it is [active](Self::set_active).
    pub fn new(backend: Rc<dyn UiBackend>, cx: &mut Context<Self>) -> RulesModel {
        let mut model = RulesModel {
            backend,
            statuses: Vec::new(),
            rows: Vec::new(),
            dropped_fraction: 0.0,
            file_error: None,
            action_error: None,
            active: false,
            editor: None,
            sessions: 0,
        };
        model.read();
        cx.spawn(async move |rules, cx| {
            loop {
                cx.background_executor().timer(RULES_REFRESH).await;
                // The model is gone once the window closes; that ends the loop.
                if rules.update(cx, |rules, cx| rules.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        model
    }

    // --- what it holds ----------------------------------------------------------------

    /// The cards, in the file's order.
    pub fn rows(&self) -> &[RuleRow] {
        &self.rows
    }

    /// The rules behind the cards, with their IDs.
    pub fn statuses(&self) -> &[RuleStatus] {
        &self.statuses
    }

    /// The line on the right of the header: `31% of incoming data dropped`, or
    /// `Nothing dropped yet`.
    pub fn dropped_line(&self) -> String {
        rules::dropped_line(self.dropped_fraction)
    }

    /// The error of the rules file, as the engine reports it, `None` when the
    /// file is fine.
    pub fn file_error(&self) -> Option<&str> {
        self.file_error.as_deref()
    }

    /// The banner for [`file_error`](Self::file_error): `The rules file has an
    /// error: <msg>. loctel keeps using the rules it had.`
    pub fn file_error_line(&self) -> Option<String> {
        self.file_error.as_deref().map(rules::file_error_line)
    }

    /// Why the last switch of a card failed (the file could not be written),
    /// `None` after the next one that worked.
    pub fn action_error(&self) -> Option<&str> {
        self.action_error.as_deref()
    }

    /// There are no rules: the list says so.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether the Rules screen is the one showing.
    pub fn active(&self) -> bool {
        self.active
    }

    /// The rule in the editor, if one is open.
    pub fn editor(&self) -> Option<&RuleEditor> {
        self.editor.as_ref()
    }

    /// The index of the card that is open in the editor: `None` with no editor,
    /// and for a rule that is not in the list (a new one, or one that a change
    /// of the file took away).
    pub fn selected(&self) -> Option<usize> {
        let target = self.editor.as_ref()?.target?;
        self.rows.iter().position(|row| row.id == target)
    }

    // --- what the user does -------------------------------------------------------------------

    /// The shell says whether the Rules screen is showing. While it is not, the
    /// timer reads nothing. Becoming active reads at once.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.refresh_now(cx);
        }
    }

    /// Reads the rules again now (the timer does it every second), and notices
    /// if they changed behind an open editor.
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        let changed = self.read();
        let flagged = self.detect_change();
        if changed || flagged {
            cx.notify();
        }
    }

    /// The switch of a card: turns the rule on or off and saves. The editor, if
    /// open, goes on: its snapshot follows (the draft of this very rule gets the
    /// new state of the switch), and this change is not an external one.
    pub fn set_enabled(&mut self, id: RuleId, on: bool, cx: &mut Context<Self>) {
        self.detect_change();
        match self.backend.set_rule_enabled(id, on) {
            Ok(()) => {
                self.action_error = None;
                self.after_own_write();
            }
            Err(error) => self.action_error = Some(save_error(&error)),
        }
        self.read();
        cx.notify();
    }

    /// `New rule`: the editor opens on a blank rule (traces, the whole trace,
    /// on), unsaved.
    pub fn new_rule(&mut self, cx: &mut Context<Self>) {
        self.open(
            None,
            Draft {
                name: String::new(),
                language: RuleLanguage::TraceQl,
                query: String::new(),
                scope: DropScope::Trace,
                enabled: true,
            },
            cx,
        );
    }

    /// A click on a card: the editor opens on that rule. An ID that is not in
    /// the list (a reload took it away) does nothing.
    pub fn edit(&mut self, id: RuleId, cx: &mut Context<Self>) {
        self.read();
        let Some(status) = self.statuses.iter().find(|status| status.id == id) else {
            cx.notify();
            return;
        };
        let draft = Draft::from_spec(&status.spec);
        self.open(Some(id), draft, cx);
    }

    /// The editor opens on `draft`, unsaved: what `Discard lines like this…`
    /// brings. A draft that was open is replaced.
    pub fn open_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        self.open(None, draft, cx);
    }

    /// The `Name` field changed.
    pub fn set_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.draft.name != name {
            editor.draft.name = name.to_owned();
            editor.error = None;
            cx.notify();
        }
    }

    /// The `Applies to` control: the language of the query. The query keeps its
    /// text (read by the other parser now), and the preview runs again.
    pub fn set_language(&mut self, language: RuleLanguage, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.draft.language != language {
            editor.draft.language = language;
            editor.error = None;
            self.schedule_preview(None, cx);
            cx.notify();
        }
    }

    /// The `Query` field changed. The preview runs once the typing stops.
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.draft.query != query {
            editor.draft.query = query.to_owned();
            editor.error = None;
            self.schedule_preview(Some(PREVIEW_DEBOUNCE), cx);
            cx.notify();
        }
    }

    /// The radio of a trace rule: what a match drops.
    pub fn set_scope(&mut self, scope: DropScope, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.draft.scope != scope {
            editor.draft.scope = scope;
            editor.error = None;
            self.schedule_preview(None, cx);
            cx.notify();
        }
    }

    /// `Save`: the rules as the editor saw them, with the draft in the place of
    /// its rule (at the end for a new one), become the rules. The editor closes
    /// when it worked; when it did not, it stays and says why. Nothing happens
    /// without a name, while the rules changed behind the editor's back and the
    /// user has not decided, or while the editor asks about a delete. A query
    /// that does not parse is saved (a draft, per M2).
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.detect_change();
        let Some(editor) = &self.editor else {
            return;
        };
        if !editor.can_save() {
            cx.notify();
            return;
        }
        let mut specs: Vec<RuleSpec> = editor.base.iter().map(|(_, spec)| spec.clone()).collect();
        let spec = editor.draft.to_spec();
        match editor.position() {
            Some(index) => specs[index] = spec,
            None => specs.push(spec),
        }
        self.write(specs, cx);
    }

    /// `Cancel`: the editor closes and nothing is saved.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
    }

    /// `Delete`: the editor asks. Nothing is removed yet. A rule that is not
    /// saved has nothing to delete.
    pub fn ask_delete(&mut self, cx: &mut Context<Self>) {
        self.detect_change();
        if let Some(editor) = &mut self.editor
            && !editor.is_new()
            && !editor.external_change
        {
            editor.confirming_delete = true;
            editor.error = None;
        }
        cx.notify();
    }

    /// The `Keep rule` of the question: back to the editor.
    pub fn keep_rule(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.confirming_delete = false;
            cx.notify();
        }
    }

    /// The yes of the question: the rules as the editor saw them, less this
    /// one, become the rules, and the editor closes.
    pub fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        self.detect_change();
        let Some(editor) = &self.editor else {
            return;
        };
        if !editor.confirming_delete || editor.external_change {
            cx.notify();
            return;
        }
        let Some(target) = editor.target else {
            return;
        };
        let specs: Vec<RuleSpec> = editor
            .base
            .iter()
            .filter(|(id, _)| *id != target)
            .map(|(_, spec)| spec.clone())
            .collect();
        self.write(specs, cx);
    }

    /// `Keep my draft`: the banner goes, the draft stays, and from now on the
    /// editor has accounted for the file as it is. `Save` then overwrites it.
    pub fn keep_draft(&mut self, cx: &mut Context<Self>) {
        let generation = self.backend.rules_generation();
        if let Some(editor) = &mut self.editor
            && editor.external_change
        {
            editor.external_change = false;
            editor.generation = generation;
            cx.notify();
        }
    }

    /// `Discard my draft`: the editor closes, and the file is as it is.
    pub fn discard_draft(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
    }

    // --- inside -------------------------------------------------------------------------------

    /// The timer: read again, while the screen shows.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.refresh_now(cx);
        }
    }

    /// Reads the rules, the share dropped and the file's error from the
    /// backend. Whether anything changed.
    fn read(&mut self) -> bool {
        let statuses = self.backend.rules();
        let status = self.backend.status();
        let changed = statuses != self.statuses
            || status.dropped_fraction != self.dropped_fraction
            || status.rules_file_error != self.file_error;
        if changed {
            self.rows = rules::rows(&statuses);
            self.statuses = statuses;
            self.dropped_fraction = status.dropped_fraction;
            self.file_error = status.rules_file_error;
        }
        changed
    }

    /// Opens the editor on `draft`, for the saved rule `target` (`None`: a rule
    /// that is not saved). The snapshot is the rules as they are now.
    fn open(&mut self, target: Option<RuleId>, draft: Draft, cx: &mut Context<Self>) {
        // The generation first: a change between the two reads is then noticed
        // (a banner too many is safe, one too few is not).
        let generation = self.backend.rules_generation();
        self.read();
        self.sessions += 1;
        self.action_error = None;
        self.editor = Some(RuleEditor {
            session: self.sessions,
            target,
            draft,
            base: self.snapshot(),
            generation,
            external_change: false,
            confirming_delete: false,
            error: None,
            preview: PreviewState::Pending,
            preview_generation: 0,
        });
        self.schedule_preview(None, cx);
        cx.notify();
    }

    /// The rules as they are, with their IDs.
    fn snapshot(&self) -> Vec<(RuleId, RuleSpec)> {
        self.statuses
            .iter()
            .map(|status| (status.id, status.spec.clone()))
            .collect()
    }

    /// Closes the editor, and reads the rules (the list may have moved on).
    fn close(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.read();
        cx.notify();
    }

    /// Whether the rules changed behind the open editor's back: the generation is
    /// not the one it has accounted for. Turns the banner on when they did.
    /// Whether the banner is on.
    fn detect_change(&mut self) -> bool {
        let generation = self.backend.rules_generation();
        match &mut self.editor {
            Some(editor) => {
                if generation != editor.generation {
                    editor.external_change = true;
                }
                editor.external_change
            }
            None => false,
        }
    }

    /// This model has just changed the rules (a switch): the editor's snapshot
    /// is the rules as they are, and the generation is the one the write caused,
    /// unless the rules had already changed behind the editor's back (then the
    /// banner stands, and the snapshot is what the user saw).
    fn after_own_write(&mut self) {
        let generation = self.backend.rules_generation();
        self.read();
        let base = self.snapshot();
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.external_change {
            return;
        }
        editor.base = base;
        editor.generation = generation;
        // The switch of the open rule's own card moved its `enabled`: the draft
        // follows, or saving it would turn the rule back on.
        if let Some(target) = editor.target
            && let Some((_, spec)) = editor.base.iter().find(|(id, _)| *id == target)
        {
            editor.draft.enabled = spec.enabled;
        }
    }

    /// Makes `specs` the rules. On success the editor closes; on failure it
    /// stays, with the reason.
    fn write(&mut self, specs: Vec<RuleSpec>, cx: &mut Context<Self>) {
        match self.backend.set_rules(specs) {
            Ok(()) => {
                self.editor = None;
                self.action_error = None;
            }
            Err(error) => {
                let message = save_error(&error);
                match &mut self.editor {
                    Some(editor) => editor.error = Some(message),
                    None => self.action_error = Some(message),
                }
            }
        }
        self.read();
        cx.notify();
    }

    /// Asks for the preview of the draft: waits `debounce` first when there is
    /// one, then runs it on the background executor. The state changes at once
    /// for a query that does not parse (no read) and for a blank one.
    fn schedule_preview(&mut self, debounce: Option<Duration>, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.preview_generation += 1;
        let (session, generation) = (editor.session, editor.preview_generation);
        if editor.draft.query.trim().is_empty() {
            editor.preview = PreviewState::Pending;
            return;
        }
        if editor.draft.validate().query.is_err() {
            editor.preview = PreviewState::Invalid;
            return;
        }
        if editor.preview == PreviewState::Invalid {
            // It parses now: the old complaint goes while the result is on its
            // way. A result that is there stays until the next one replaces it.
            editor.preview = PreviewState::Pending;
        }
        let spec = editor.draft.to_spec();
        let language = spec.language;
        let store = self.backend.store();
        cx.spawn(async move |rules, cx| {
            if let Some(wait) = debounce {
                cx.background_executor().timer(wait).await;
                let current = rules
                    .update(cx, |rules, _| rules.preview_is_current(session, generation))
                    .unwrap_or(false);
                if !current {
                    return;
                }
            }
            let result = cx
                .background_executor()
                .spawn(async move { run_preview(&spec, &store) })
                .await;
            rules
                .update(cx, |rules, cx| {
                    rules.apply_preview(session, generation, language, result, cx)
                })
                .ok();
        })
        .detach();
    }

    /// Whether the preview asked for as `generation` in editor `session` is still
    /// the one wanted.
    fn preview_is_current(&self, session: u64, generation: u64) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|e| e.session == session && e.preview_generation == generation)
    }

    fn apply_preview(
        &mut self,
        session: u64,
        generation: u64,
        language: RuleLanguage,
        result: Result<Preview, QueryError>,
        cx: &mut Context<Self>,
    ) {
        if !self.preview_is_current(session, generation) {
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.preview = match result {
                Ok(preview) => PreviewState::Ready { language, preview },
                Err(_) => PreviewState::Invalid,
            };
            cx.notify();
        }
    }
}

/// What a rule would have dropped of the last [`PREVIEW_WINDOW`] of `store`, on
/// the store's own clock: what `UiBackend::preview_rule` is, as a function of
/// what can leave the foreground thread.
fn run_preview(spec: &RuleSpec, store: &Store) -> Result<Preview, QueryError> {
    let window = u64::try_from(PREVIEW_WINDOW.as_nanos()).unwrap_or(u64::MAX);
    let since = store.now_unix_nano().saturating_sub(window);
    loctel_rules::preview(spec, store, since)
}

/// The words for a rules file that could not be written.
fn save_error(error: &anyhow::Error) -> String {
    format!("Could not save the rules: {error:#}")
}
