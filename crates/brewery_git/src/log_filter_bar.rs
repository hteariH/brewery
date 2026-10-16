//! The filter bar above the Log tool window's graph: Branch, User, Date, Paths, "Text or hash",
//! Reset, and a "Filtered" chip with the commit count.
//!
//! The bar owns the `LogFilter` and emits `LogFilterChanged` whenever it changes; the panel
//! rebuilds the graph from it and reports back the count through `set_status`.

use crate::GitCli;
use crate::log_filter::{DateBound, DatePreset, LogFilter, branch_choices};
use editor::Editor;
use git::repository::RepoPath;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, Task, WeakEntity, Window,
};
use picker::{Picker, PickerDelegate};
use project::git_store::Repository;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use ui::{
    Checkbox, Chip, ListItem, ListItemSpacing, PopoverMenu, ToggleState, Tooltip, prelude::*,
};

pub const REMOTE_TOOLTIP: &str = "Filters work in local projects";
const TEXT_TOOLTIP: &str = "Text in commit messages, ignoring case, or a commit hash. While a \
    text or user filter is set, the Log search ignores case too.";
/// How many recent commits the User choices are collected from.
const AUTHOR_SCAN_LIMIT: usize = 2000;
const GIT_TIMEOUT: Duration = Duration::from_secs(20);

/// The filter changed; read it with `LogFilterBar::filter`.
pub struct LogFilterChanged;

/// A commit author, or the current git user.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Person {
    pub name: String,
    pub email: String,
}

impl Person {
    /// Parses `Name <email>` (git's `%aN <%aE>`).
    pub fn parse(ident: &str) -> Option<Self> {
        let (name, email) = ident.trim().rsplit_once(" <")?;
        let email = email.strip_suffix('>')?;
        let name = name.trim();
        (!name.is_empty()).then(|| Self {
            name: name.to_string(),
            email: email.trim().to_string(),
        })
    }

    /// The `--author` value that matches this person only. Authors match as fixed substrings
    /// of `Name <email>`, so `<email>` with its brackets is an exact email match; without an
    /// email, `Name <` at least anchors the end of the name.
    pub fn filter_value(&self) -> String {
        if self.email.is_empty() {
            format!("{} <", self.name)
        } else {
            format!("<{}>", self.email)
        }
    }

    pub fn ident(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

/// What the panel reports about the filtered graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterStatus {
    pub count: usize,
    pub loading: bool,
}

pub struct LogFilterBar {
    filter: LogFilter,
    repo: Option<Entity<Repository>>,
    enabled: bool,
    /// `user.name` and `user.email` from git config: the "me" User choice.
    me: Option<Person>,
    /// Authors of recent commits, sorted, one per identity.
    authors: Vec<Person>,
    status: Option<FilterStatus>,
    error: Option<SharedString>,
    text_editor: Entity<Editor>,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LogFilterChanged> for LogFilterBar {}

impl LogFilterBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Text or hash", window, cx);
            editor
        });
        Self {
            filter: LogFilter::default(),
            repo: None,
            enabled: true,
            me: None,
            authors: Vec::new(),
            status: None,
            error: None,
            text_editor,
            _load: Task::ready(()),
            _subscriptions: Vec::new(),
        }
    }

    pub fn filter(&self) -> &LogFilter {
        &self.filter
    }

    pub fn me(&self) -> Option<&Person> {
        self.me.as_ref()
    }

    pub fn authors(&self) -> &[Person] {
        &self.authors
    }

    #[cfg(test)]
    pub(crate) fn user_label_for_test(&self) -> String {
        self.user_label()
    }

    #[cfg(test)]
    pub(crate) fn text_editor_for_test(&self) -> Entity<Editor> {
        self.text_editor.clone()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn status(&self) -> Option<FilterStatus> {
        self.status
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// "Filtered · 3 commits" once a filtered graph has loaded.
    pub fn status_text(&self) -> Option<SharedString> {
        let status = self.status?;
        if status.loading {
            return Some("Filtered · loading…".into());
        }
        let noun = if status.count == 1 {
            "commit"
        } else {
            "commits"
        };
        Some(format!("Filtered · {} {noun}", status.count).into())
    }

    /// Filters are local only (`LogSource::Args` has no proto form).
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.enabled = enabled;
        cx.notify();
    }

    pub fn set_status(&mut self, status: Option<FilterStatus>, cx: &mut Context<Self>) {
        if self.status != status {
            self.status = status;
            cx.notify();
        }
    }

    pub fn set_error(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    /// The repository the choices come from. Switching to another repository resets the
    /// filter; a filter set before the first repository is kept.
    pub fn set_repository(
        &mut self,
        repo: Option<Entity<Repository>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repo.as_ref().map(Entity::entity_id) == repo.as_ref().map(Entity::entity_id) {
            return;
        }
        let had_repo = self.repo.is_some();
        self.repo = repo;
        self.me = None;
        self.authors.clear();
        self.status = None;
        self.error = None;
        if had_repo && !self.filter.is_empty() {
            self.filter = LogFilter::default();
            self.text_editor
                .update(cx, |editor, cx| editor.set_text("", window, cx));
            cx.emit(LogFilterChanged);
        }
        self._load = self.load_choices(cx);
        cx.notify();
    }

    fn load_choices(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(repo) = self.repo.clone() else {
            return Task::ready(());
        };
        if !self.enabled {
            return Task::ready(());
        }
        let config = |key: &str, cx: &mut Context<Self>| {
            GitCli::run(
                &repo,
                vec!["config".into(), "--get".into(), key.into()],
                Some(GIT_TIMEOUT),
                cx,
            )
        };
        let me_name = config("user.name", cx);
        let me_email = config("user.email", cx);
        let authors = GitCli::run(
            &repo,
            [
                "log",
                "--format=%aN <%aE>",
                &format!("--max-count={AUTHOR_SCAN_LIMIT}"),
                "--ignore-missing",
                "--branches",
                "--remotes",
                "--tags",
                "HEAD",
                "--",
            ]
            .map(String::from)
            .to_vec(),
            Some(GIT_TIMEOUT),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let value = async |task: Task<anyhow::Result<crate::GitOutput>>| match task.await {
                Ok(output) if output.success() => {
                    Some(output.stdout.trim().to_string()).filter(|value| !value.is_empty())
                }
                Ok(_) => None,
                Err(error) => {
                    log::debug!("git config for the Log filter: {error:#}");
                    None
                }
            };
            let name = value(me_name).await;
            let email = value(me_email).await;
            let me = name.map(|name| Person {
                name,
                email: email.unwrap_or_default(),
            });
            let authors = match authors.await {
                Ok(output) if output.success() => output
                    .stdout
                    .lines()
                    .filter_map(Person::parse)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                Ok(output) => {
                    log::debug!("authors for the Log filter: {}", output.message());
                    Vec::new()
                }
                Err(error) => {
                    log::debug!("authors for the Log filter: {error:#}");
                    Vec::new()
                }
            };
            this.update(cx, |this, cx| {
                this.me = me;
                this.authors = authors;
                cx.notify();
            })
            .ok();
        })
    }

    /// Replaces the whole filter.
    pub fn set_filter(&mut self, filter: LogFilter, window: &mut Window, cx: &mut Context<Self>) {
        let text = filter.text.clone().unwrap_or_default();
        if self.text_editor.read(cx).text(cx) != text {
            self.text_editor
                .update(cx, |editor, cx| editor.set_text(text, window, cx));
        }
        self.change(|filter_slot| *filter_slot = filter, cx);
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_filter(LogFilter::default(), window, cx);
    }

    fn change(&mut self, edit: impl FnOnce(&mut LogFilter), cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        edit(&mut filter);
        if filter != self.filter {
            self.filter = filter;
            self.error = None;
            cx.emit(LogFilterChanged);
            cx.notify();
        }
    }

    pub fn toggle_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if name.starts_with('-') {
            // git would read it as an option: never let it into the filter.
            self.error = Some(format!("not a branch name: {name}").into());
            cx.notify();
            return;
        }
        self.change(|filter| toggle(&mut filter.branches, name), cx);
    }

    pub fn toggle_author(&mut self, author: &str, cx: &mut Context<Self>) {
        let author = author.trim().to_string();
        if author.is_empty() {
            return;
        }
        self.change(|filter| toggle(&mut filter.authors, author), cx);
    }

    pub fn set_since(&mut self, since: Option<DateBound>, cx: &mut Context<Self>) {
        self.change(|filter| filter.since = since, cx);
    }

    pub fn set_until(&mut self, until: Option<DateBound>, cx: &mut Context<Self>) {
        self.change(|filter| filter.until = until, cx);
    }

    pub fn set_preset(&mut self, preset: DatePreset, cx: &mut Context<Self>) {
        let since = preset.since(OffsetDateTime::now_utc());
        self.change(
            |filter| {
                filter.since = Some(since);
                filter.until = None;
            },
            cx,
        );
    }

    /// One path per line, repository-relative; absolute paths inside the repository are
    /// accepted too. Returns the lines that are not paths of this repository.
    pub fn set_paths_text(&mut self, text: &str, cx: &mut Context<Self>) -> Vec<String> {
        let work_dir = self
            .repo
            .as_ref()
            .map(|repo| repo.read(cx).work_directory_abs_path.clone());
        let mut paths = Vec::new();
        let mut rejected = Vec::new();
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            match parse_path(line, work_dir.as_deref()) {
                Some(path) if !paths.contains(&path) => paths.push(path),
                Some(_) => {}
                None => rejected.push(line.to_string()),
            }
        }
        self.change(|filter| filter.paths = paths, cx);
        if !rejected.is_empty() {
            self.error =
                Some(format!("Not a path in this repository: {}", rejected.join(", ")).into());
            cx.notify();
        }
        rejected
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = Some(text.trim().to_string()).filter(|text| !text.is_empty());
        self.change(
            |filter| {
                if filter.text != text {
                    filter.text = text;
                    filter.text_is_hash = None;
                }
            },
            cx,
        );
    }

    /// Marks the hash-like text as not a commit, so it greps the messages instead; ignored
    /// when the text changed since it was looked up.
    pub fn set_text_not_a_commit(&mut self, text: &str, cx: &mut Context<Self>) {
        self.change(
            |filter| {
                if filter.text.as_deref().map(str::trim) == Some(text) {
                    filter.text_is_hash = Some(false);
                }
            },
            cx,
        );
    }

    fn confirm_text(&mut self, _: &menu::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.text_editor.read(cx).text(cx);
        self.set_text(&text, cx);
    }

    fn branch_label(&self) -> String {
        summary("Branch", &self.filter.branches)
    }

    fn user_label(&self) -> String {
        let names = self
            .filter
            .authors
            .iter()
            .map(|value| self.author_name(value))
            .collect::<Vec<_>>();
        summary("User", &names)
    }

    /// The name to show for an author filter value (`<email>` for picked people).
    fn author_name(&self, value: &str) -> String {
        if self
            .me
            .as_ref()
            .is_some_and(|me| me.filter_value() == value)
        {
            return "me".into();
        }
        self.authors
            .iter()
            .find(|person| person.filter_value() == value)
            .map_or_else(|| value.to_string(), |person| person.name.clone())
    }

    fn date_label(&self) -> String {
        match (&self.filter.since, &self.filter.until) {
            (None, None) => "Date: All".into(),
            (Some(since), None) => format!("Date: since {}", since.label()),
            (None, Some(until)) => format!("Date: until {}", until.label()),
            (Some(since), Some(until)) => format!("Date: {} – {}", since.label(), until.label()),
        }
    }

    fn paths_label(&self) -> String {
        let paths = self
            .filter
            .paths
            .iter()
            .map(|path| path.as_unix_str().to_string())
            .collect::<Vec<_>>();
        summary("Paths", &paths)
    }

    pub(crate) fn choice_picker(
        &self,
        kind: ChoiceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Picker<ChoiceDelegate>> {
        let (choices, checked) = match kind {
            ChoiceKind::Branch => {
                let names = self
                    .repo
                    .as_ref()
                    .map(|repo| branch_choices(&repo.read(cx).branch_list))
                    .unwrap_or_default();
                let choices = names
                    .into_iter()
                    .map(|name| Choice {
                        label: name.clone().into(),
                        value: name,
                    })
                    .collect();
                (choices, self.filter.branches.clone())
            }
            ChoiceKind::User => {
                let mut choices = Vec::new();
                if let Some(me) = &self.me {
                    choices.push(Choice {
                        label: format!("me ({})", me.ident()).into(),
                        value: me.filter_value(),
                    });
                }
                let me_value = self.me.as_ref().map(Person::filter_value);
                for author in &self.authors {
                    let value = author.filter_value();
                    if Some(&value) != me_value.as_ref()
                        && !choices.iter().any(|c| c.value == value)
                    {
                        choices.push(Choice {
                            label: author.ident().into(),
                            value,
                        });
                    }
                }
                (choices, self.filter.authors.clone())
            }
        };
        let delegate = ChoiceDelegate {
            bar: cx.entity().downgrade(),
            kind,
            choices,
            checked,
            rows: Vec::new(),
            selected_index: 0,
        };
        cx.new(|cx| Picker::uniform_list(delegate, window, cx).max_height(rems(20.)))
    }

    fn filter_button(&self, id: &'static str, label: String, active: bool) -> Button {
        let button = Button::new(id, label)
            .label_size(LabelSize::Small)
            .color(if active {
                Color::Accent
            } else {
                Color::Default
            })
            .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
            .disabled(!self.enabled);
        if self.enabled {
            button
        } else {
            button.tooltip(Tooltip::text(REMOTE_TOOLTIP))
        }
    }
}

fn toggle(values: &mut Vec<String>, value: String) {
    if let Some(ix) = values.iter().position(|existing| *existing == value) {
        values.remove(ix);
    } else {
        values.push(value);
    }
}

fn summary(name: &str, values: &[String]) -> String {
    match values {
        [] => format!("{name}: All"),
        [one] => format!("{name}: {one}"),
        [first, rest @ ..] => format!("{name}: {first} +{}", rest.len()),
    }
}

fn parse_path(line: &str, work_dir: Option<&std::path::Path>) -> Option<RepoPath> {
    let as_path = std::path::Path::new(line);
    if as_path.is_absolute() {
        let relative = as_path.strip_prefix(work_dir?).ok()?;
        let text = relative.to_string_lossy().replace('\\', "/");
        return RepoPath::new(text.trim_end_matches('/')).ok();
    }
    let text = line.replace('\\', "/");
    let text = text.trim_start_matches("./").trim_end_matches('/');
    if text.is_empty() || text.split('/').any(|part| part == "..") {
        return None;
    }
    RepoPath::new(text).ok()
}

impl Render for LogFilterBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.entity().downgrade();
        let filter = &self.filter;
        let branch = {
            let bar = bar.clone();
            PopoverMenu::new("log-filter-branch-menu")
                .trigger(self.filter_button(
                    "log-filter-branch",
                    self.branch_label(),
                    !filter.branches.is_empty(),
                ))
                .menu(move |window, cx| {
                    bar.upgrade().map(|bar| {
                        bar.update(cx, |bar, cx| {
                            bar.choice_picker(ChoiceKind::Branch, window, cx)
                        })
                    })
                })
        };
        let user = {
            let bar = bar.clone();
            PopoverMenu::new("log-filter-user-menu")
                .trigger(self.filter_button(
                    "log-filter-user",
                    self.user_label(),
                    !filter.authors.is_empty(),
                ))
                .menu(move |window, cx| {
                    bar.upgrade().map(|bar| {
                        bar.update(cx, |bar, cx| {
                            bar.choice_picker(ChoiceKind::User, window, cx)
                        })
                    })
                })
        };
        let date = {
            let bar = bar.clone();
            PopoverMenu::new("log-filter-date-menu")
                .trigger(self.filter_button(
                    "log-filter-date",
                    self.date_label(),
                    filter.since.is_some() || filter.until.is_some(),
                ))
                .menu(move |window, cx| {
                    let bar = bar.clone();
                    Some(cx.new(|cx| DateMenu::new(bar, window, cx)))
                })
        };
        let paths = {
            let current = filter
                .paths
                .iter()
                .map(|path| path.as_unix_str().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            PopoverMenu::new("log-filter-paths-menu")
                .trigger(self.filter_button(
                    "log-filter-paths",
                    self.paths_label(),
                    !filter.paths.is_empty(),
                ))
                .menu(move |window, cx| {
                    let bar = bar.clone();
                    let current = current.clone();
                    Some(cx.new(|cx| PathsMenu::new(bar, &current, window, cx)))
                })
        };
        let text = h_flex()
            .id("log-filter-text")
            .key_context("LogFilterText")
            .on_action(cx.listener(Self::confirm_text))
            .w(rems(14.))
            .h_6()
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .when(self.enabled, |this| {
                this.child(self.text_editor.clone())
                    .tooltip(Tooltip::text(TEXT_TOOLTIP))
            })
            .when(!self.enabled, |this| {
                this.child(
                    Label::new("Text or hash")
                        .color(Color::Disabled)
                        .size(LabelSize::Small),
                )
                .tooltip(Tooltip::text(REMOTE_TOOLTIP))
            });

        h_flex()
            .w_full()
            .flex_none()
            .px_1()
            .py_0p5()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(text)
            .child(branch)
            .child(user)
            .child(date)
            .child(paths)
            .when(!self.filter.is_empty(), |this| {
                this.child(
                    Button::new("log-filter-reset", "Reset")
                        .label_size(LabelSize::Small)
                        .start_icon(Icon::new(IconName::Close).size(IconSize::XSmall))
                        .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx))),
                )
            })
            .when_some(self.status_text(), |this, status| {
                this.child(Chip::new(status).label_color(Color::Accent))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).color(Color::Error).size(LabelSize::Small))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKind {
    Branch,
    User,
}

struct Choice {
    label: SharedString,
    value: String,
}

enum ChoiceRow {
    Choice(usize),
    /// The typed text, when no choice is exactly it.
    Typed(String),
}

/// A multi-select list with search: Enter or a click toggles the row and keeps the list open.
pub struct ChoiceDelegate {
    bar: WeakEntity<LogFilterBar>,
    kind: ChoiceKind,
    choices: Vec<Choice>,
    checked: Vec<String>,
    rows: Vec<ChoiceRow>,
    selected_index: usize,
}

impl ChoiceDelegate {
    fn row_value(&self, row: &ChoiceRow) -> String {
        match row {
            ChoiceRow::Choice(ix) => self.choices[*ix].value.clone(),
            ChoiceRow::Typed(text) => text.clone(),
        }
    }
}

impl PickerDelegate for ChoiceDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "brewery log filter choices"
    }

    fn match_count(&self) -> usize {
        self.rows.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        match self.kind {
            ChoiceKind::Branch => "Search branches, or type a ref".into(),
            ChoiceKind::User => "Search users, or type a name or email".into(),
        }
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let query = query.trim();
        let needle = query.to_lowercase();
        let mut rows = self
            .choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| {
                choice.label.to_lowercase().contains(&needle)
                    || choice.value.to_lowercase().contains(&needle)
            })
            .map(|(ix, _)| ChoiceRow::Choice(ix))
            .collect::<Vec<_>>();
        if !query.is_empty()
            && !self
                .choices
                .iter()
                .any(|choice| choice.value.eq_ignore_ascii_case(query))
        {
            rows.push(ChoiceRow::Typed(query.to_string()));
        }
        // Checked values not among the choices (typed earlier) stay visible and uncheckable.
        if query.is_empty() {
            for value in &self.checked {
                if !self.choices.iter().any(|choice| &choice.value == value) {
                    rows.push(ChoiceRow::Typed(value.clone()));
                }
            }
        }
        self.rows = rows;
        self.selected_index = self.selected_index.min(self.rows.len().saturating_sub(1));
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(row) = self.rows.get(self.selected_index) else {
            return;
        };
        let value = self.row_value(row);
        toggle(&mut self.checked, value.clone());
        let kind = self.kind;
        self.bar
            .update(cx, |bar, cx| match kind {
                ChoiceKind::Branch => bar.toggle_branch(&value, cx),
                ChoiceKind::User => bar.toggle_author(&value, cx),
            })
            .ok();
        if matches!(row, ChoiceRow::Typed(_)) {
            cx.defer_in(window, |picker, window, cx| {
                picker.set_query("", window, cx);
            });
        }
        cx.notify();
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let row = self.rows.get(ix)?;
        let value = self.row_value(row);
        let checked = self.checked.contains(&value);
        let label: SharedString = match row {
            ChoiceRow::Choice(choice) => self.choices[*choice].label.clone(),
            ChoiceRow::Typed(text) if checked => text.clone().into(),
            ChoiceRow::Typed(text) => format!("Use “{text}”").into(),
        };
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(Checkbox::new(
                    ("log-filter-choice", ix),
                    ToggleState::from(checked),
                ))
                .child(Label::new(label)),
        )
    }
}

/// Presets plus "Since" / "Until" days (YYYY-MM-DD, applied on Enter).
struct DateMenu {
    bar: WeakEntity<LogFilterBar>,
    since: Entity<Editor>,
    until: Entity<Editor>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl DateMenu {
    fn new(bar: WeakEntity<LogFilterBar>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (since_text, until_text) = bar
            .upgrade()
            .map(|bar| {
                let filter = bar.read(cx).filter();
                let day = |bound: &Option<DateBound>| match bound {
                    Some(DateBound::Day(_)) => bound.map(|b| b.label()).unwrap_or_default(),
                    _ => String::new(),
                };
                (day(&filter.since), day(&filter.until))
            })
            .unwrap_or_default();
        let editor =
            |placeholder: &str, text: String, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text(placeholder, window, cx);
                    editor.set_text(text, window, cx);
                    editor
                })
            };
        let since = editor("Since… YYYY-MM-DD", since_text, window, cx);
        let until = editor("Until… YYYY-MM-DD", until_text, window, cx);
        let focus_handle = since.focus_handle(cx);
        Self {
            bar,
            since,
            until,
            error: None,
            focus_handle,
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let parse = |editor: &Entity<Editor>, cx: &App| -> Result<Option<DateBound>, ()> {
            let text = editor.read(cx).text(cx);
            if text.trim().is_empty() {
                return Ok(None);
            }
            DateBound::parse_day(&text).map(Some).ok_or(())
        };
        match (parse(&self.since, cx), parse(&self.until, cx)) {
            (Ok(since), Ok(until)) => {
                self.bar
                    .update(cx, |bar, cx| {
                        bar.set_since(since, cx);
                        bar.set_until(until, cx);
                    })
                    .ok();
                cx.emit(DismissEvent);
            }
            _ => {
                self.error = Some("Use YYYY-MM-DD".into());
                cx.notify();
            }
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for DateMenu {}

impl Focusable for DateMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DateMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preset = |preset: DatePreset, cx: &mut Context<Self>| {
            Button::new(preset.label(), preset.label())
                .full_width()
                .label_size(LabelSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.bar
                        .update(cx, |bar, cx| bar.set_preset(preset, cx))
                        .ok();
                    cx.emit(DismissEvent);
                }))
        };
        let field = |editor: &Entity<Editor>, cx: &mut Context<Self>| {
            h_flex()
                .h_6()
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(cx.theme().colors().border_variant)
                .child(editor.clone())
        };
        v_flex()
            .key_context("LogFilterDate")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .elevation_2(cx)
            .p_1()
            .gap_1()
            .w(rems(16.))
            .child(
                Button::new("log-filter-any-time", "Any time")
                    .full_width()
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bar
                            .update(cx, |bar, cx| {
                                bar.set_since(None, cx);
                                bar.set_until(None, cx);
                            })
                            .ok();
                        cx.emit(DismissEvent);
                    })),
            )
            .children(DatePreset::ALL.map(|p| preset(p, cx)))
            .child(field(&self.since.clone(), cx))
            .child(field(&self.until.clone(), cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).color(Color::Error).size(LabelSize::Small))
            })
    }
}

/// Paths, one per line, applied with the Apply button.
struct PathsMenu {
    bar: WeakEntity<LogFilterBar>,
    editor: Entity<Editor>,
}

impl PathsMenu {
    fn new(
        bar: WeakEntity<LogFilterBar>,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(3, 10, window, cx);
            editor.set_placeholder_text("Paths, one per line", window, cx);
            editor.set_text(current, window, cx);
            editor
        });
        Self { bar, editor }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text(cx);
        let rejected = self
            .bar
            .update(cx, |bar, cx| bar.set_paths_text(&text, cx))
            .unwrap_or_default();
        if rejected.is_empty() {
            cx.emit(DismissEvent);
        }
    }
}

impl EventEmitter<DismissEvent> for PathsMenu {}

impl Focusable for PathsMenu {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for PathsMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("LogFilterPaths")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .elevation_2(cx)
            .p_1()
            .gap_1()
            .w(rems(22.))
            .child(
                div()
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(self.editor.clone()),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("log-filter-paths-apply", "Apply")
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                ),
            )
    }
}
