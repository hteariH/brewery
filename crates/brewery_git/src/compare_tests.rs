use super::*;
use crate::branches_popup::{BranchAction, BranchesPopup, Row, SHOWN_TOASTS};
use crate::revision_diff_view::ResolvedSide;
use fs::RealFs;
use git::repository::CommitSummary;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use workspace::{AppState, Item as _, MultiWorkspace};
use worktree::WorktreeModelHandle;

// ---------------------------------------------------------------------------------------------
// Rows

fn branch(ref_name: &str) -> Branch {
    Branch {
        is_head: false,
        ref_name: ref_name.to_string().into(),
        upstream: None,
        most_recent_commit: Some(CommitSummary {
            sha: "abc123".into(),
            commit_timestamp: 1000,
            author_name: "Test".into(),
            subject: "subject".into(),
            has_parent: true,
        }),
    }
}

fn sample_refs() -> RefList {
    RefList::from_branches(
        &[
            branch("refs/heads/main"),
            branch("refs/heads/feature"),
            branch("refs/remotes/origin/HEAD"),
            branch("refs/remotes/origin/main"),
            branch("refs/remotes/origin/feature"),
        ],
        vec!["v2.0".into(), "v1.0".into()],
    )
}

/// One line per row: `# header`, `  ref [group]`, `? revision`; `>` marks the selection.
fn describe(rows: &CompareRows) -> Vec<String> {
    rows.rows
        .iter()
        .enumerate()
        .map(|(ix, row)| {
            let mark = if rows.selected == Some(ix) { ">" } else { " " };
            match row {
                CompareRow::Header(group) => format!("# {}", group.title()),
                CompareRow::Ref { name, group, .. } => format!("{mark} {name} [{group:?}]"),
                CompareRow::Revision(_) => format!("?{mark}{}", row.label()),
            }
        })
        .collect()
}

#[test]
fn rows_are_grouped_into_local_remote_and_tags() {
    assert_eq!(
        describe(&compare_rows(&sample_refs(), "")),
        vec![
            "# Local Branches",
            "> feature [Local]",
            "  main [Local]",
            "# Remote Branches",
            "  origin/feature [Remote]",
            "  origin/main [Remote]",
            "# Tags",
            "  v1.0 [Tags]",
            "  v2.0 [Tags]",
        ]
    );
}

#[test]
fn typed_text_filters_and_adds_a_revision_row() {
    // A partial name: matches in every group, best one selected, the text as a revision last.
    let rows = compare_rows(&sample_refs(), "feat");
    assert_eq!(
        describe(&rows),
        vec![
            "# Local Branches",
            "> feature [Local]",
            "# Remote Branches",
            "  origin/feature [Remote]",
            "? Use 'feat' as revision",
        ]
    );
    let CompareRow::Ref { positions, .. } = &rows.rows[1] else {
        panic!("a ref row");
    };
    assert_eq!(positions, &vec![0, 1, 2, 3]);

    // Nothing matches: only the revision row, selected.
    assert_eq!(
        describe(&compare_rows(&sample_refs(), "zzz")),
        vec!["?>Use 'zzz' as revision"]
    );

    // An exact ref name needs no revision row.
    assert_eq!(
        describe(&compare_rows(&sample_refs(), "v1.0")),
        vec!["# Tags", "> v1.0 [Tags]"]
    );

    // A sha or a revision expression selects the revision row even when refs match.
    let rows = compare_rows(&sample_refs(), "main~1");
    assert_eq!(
        rows.rows.last(),
        Some(&CompareRow::Revision("main~1".into()))
    );
    assert_eq!(rows.selected, Some(rows.rows.len() - 1));
    let rows = compare_rows(&sample_refs(), "abcdef12");
    assert_eq!(rows.selected, Some(rows.rows.len() - 1));
}

// ---------------------------------------------------------------------------------------------
// gpui, on a real repository

/// A real git repository in a temp dir, isolated from the user's global and system config.
struct TestRepo {
    _dir: TempDir,
    work: PathBuf,
    env: HashMap<String, String>,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let work = dir.path().join("repo");
        std::fs::create_dir(&work).expect("repo dir");
        let global = dir.path().join("gitconfig");
        std::fs::write(&global, "").expect("global config");
        let env = HashMap::from([
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                global.to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ]);
        let repo = Self {
            _dir: dir,
            work,
            env,
        };
        repo.git(&["init", "--quiet"]);
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("user.name", "Brewery Test"),
            ("user.email", "test@brewery.dev"),
            ("commit.gpgsign", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .args(args)
                .output(),
        )
        .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(&self, path: &str, content: &str, message: &str) -> String {
        std::fs::write(self.work.join(path), content).expect("write file");
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// `main` and `feature` forked from a base commit, each with its own change; `v1` tags the
    /// base. `main` is checked out.
    fn with_two_branches() -> Self {
        let repo = Self::new();
        repo.commit("a.txt", "base\n", "base");
        repo.git(&["tag", "v1"]);
        repo.git(&["checkout", "--quiet", "-b", "feature"]);
        repo.commit("a.txt", "feature\n", "feature change");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit("b.txt", "main\n", "main change");
        repo
    }
}

struct Fixture {
    workspace: Entity<Workspace>,
    repository: Entity<Repository>,
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        git_ui::init(cx);
        super::init(cx);
    });
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
}

fn shown_toasts() -> Vec<OutcomeToast> {
    SHOWN_TOASTS.with(|shown| shown.borrow().clone())
}

async fn open_real<'a>(
    repo: &TestRepo,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    crate::cli::set_test_env(repo.env.clone());
    let project = Project::test(RealFs::new(None, cx.executor()), [repo.work.as_path()], cx).await;
    let tree = project.read_with(cx, |project, cx| {
        project.worktrees(cx).next().expect("worktree")
    });
    tree.flush_fs_events(cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;
    cx.executor().run_until_parked();
    let repository = project.read_with(cx, |project, cx| {
        project.active_repository(cx).expect("repository")
    });
    let (multi_workspace, cx) = cx.add_window_view({
        let project = project.clone();
        |window, cx| MultiWorkspace::test_new(project, window, cx)
    });
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    cx.run_until_parked();
    let fixture = Fixture {
        workspace,
        repository,
    };
    wait_until(cx, "branch list", |cx| {
        fixture
            .repository
            .read_with(cx, |repo, _| repo.branch_list.len() == 2)
    });
    (fixture, cx)
}

/// Polls (real time: git runs as real processes) until `done` holds.
fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
        cx.executor().advance_clock(Duration::from_millis(200));
    }
}

fn repo_path(name: &str) -> RepoPath {
    RepoPath::new(name).expect("repo path")
}

async fn open_editor(fixture: &Fixture, name: &str, cx: &mut VisualTestContext) {
    let path = fixture.repository.read_with(cx, |repo, cx| {
        repo.repo_path_to_project_path(&repo_path(name), cx)
            .expect("project path")
    });
    fixture
        .workspace
        .update_in(cx, |workspace, window, cx| {
            workspace.open_path(path, None, true, window, cx)
        })
        .await
        .expect("open editor");
    cx.run_until_parked();
}

/// Dispatches `CompareWithRevision` from the focused element (or the active pane) and returns
/// the picker it opens.
fn open_picker(fixture: &Fixture, cx: &mut VisualTestContext) -> Entity<Picker<CompareDelegate>> {
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        if window.focused(cx).is_none() {
            window.focus(&workspace.active_pane().focus_handle(cx), cx);
        }
    });
    cx.dispatch_action(CompareWithRevision);
    cx.run_until_parked();
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_modal::<Picker<CompareDelegate>>(cx)
            .expect("the picker is open")
    })
}

fn picker_rows(picker: &Entity<Picker<CompareDelegate>>, cx: &VisualTestContext) -> Vec<String> {
    picker.read_with(cx, |picker, _| {
        picker
            .delegate
            .rows()
            .iter()
            .map(CompareRow::label)
            .collect()
    })
}

fn type_and_confirm(
    picker: &Entity<Picker<CompareDelegate>>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    picker.update_in(cx, |picker, window, cx| picker.set_query(text, window, cx));
    cx.run_until_parked();
    cx.dispatch_action(menu::Confirm);
}

fn modal_open(fixture: &Fixture, cx: &VisualTestContext) -> bool {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_modal::<Picker<CompareDelegate>>(cx)
            .is_some()
    })
}

fn active_branch_diff_title(fixture: &Fixture, cx: &VisualTestContext) -> Option<String> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        let diff = workspace.active_item_as::<BranchDiff>(cx)?;
        Some(diff.read(cx).tab_content_text(0, cx).to_string())
    })
}

/// The text the active `BranchDiff` shows (every changed file's excerpts).
fn active_branch_diff_text(fixture: &Fixture, cx: &VisualTestContext) -> String {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_item_as::<BranchDiff>(cx)
            .map(|diff| {
                diff.read(cx)
                    .editor(cx)
                    .read(cx)
                    .rhs_editor()
                    .read(cx)
                    .text(cx)
            })
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn validation_refuses_unknown_and_option_like_revisions(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    let (fixture, cx) = open_real(&repo, cx).await;

    let repository = fixture.repository.clone();
    let mut results = Vec::new();
    for rev in ["feature", "v1", "HEAD~1", "no-such-rev", "--output=x", "  "] {
        let task = cx.update(|_, cx| validate_revision(&repository, rev, cx));
        results.push(match task.await {
            Ok(sha) => sha.to_string(),
            Err(error) => format!("{error:#}"),
        });
    }
    assert_eq!(
        results,
        vec![
            repo.git(&["rev-parse", "feature"]),
            repo.git(&["rev-parse", "v1^{commit}"]),
            repo.git(&["rev-parse", "HEAD~1"]),
            "'no-such-rev' is not a branch, tag or commit".to_string(),
            "refusing revision \"--output=x\": it starts with '-'".to_string(),
            "empty revision".to_string(),
        ]
    );
}

#[gpui::test]
async fn file_target_opens_a_revision_diff(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    let (fixture, cx) = open_real(&repo, cx).await;
    open_editor(&fixture, "a.txt", cx).await;

    let picker = open_picker(&fixture, cx);
    picker.read_with(cx, |picker, _| {
        assert_eq!(
            picker.delegate.target(),
            &CompareTarget::File {
                repo: fixture.repository.clone(),
                path: repo_path("a.txt"),
            }
        );
    });
    wait_until(cx, "tags", |cx| {
        picker_rows(&picker, cx).contains(&"v1".to_string())
    });
    assert_eq!(
        picker_rows(&picker, cx),
        vec!["Local Branches", "feature", "main", "Tags", "v1"]
    );

    type_and_confirm(&picker, "feature", cx);
    let workspace = fixture.workspace.clone();
    wait_until(cx, "the revision diff", |cx| {
        workspace.read_with(cx, |workspace, cx| {
            workspace.active_item_as::<RevisionDiffView>(cx).is_some()
        })
    });
    let view = workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_item_as::<RevisionDiffView>(cx)
            .expect("view")
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.path(), &repo_path("a.txt"));
        assert_eq!(
            view.old_side(),
            &ResolvedSide::Revision {
                rev: "refs/heads/feature".into(),
                sha: repo.git(&["rev-parse", "feature"]).parse().expect("sha"),
            }
        );
        assert_eq!(view.new_side(), &ResolvedSide::WorkingCopy);
    });
    assert!(!modal_open(&fixture, cx), "the picker closed");
    assert!(shown_toasts().is_empty());
}

#[gpui::test]
async fn a_listed_branch_is_compared_even_when_a_tag_has_its_name(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    // A tag `feature` on the base commit, beside the branch `feature`.
    repo.git(&["tag", "feature", "v1"]);
    let branch_tip = repo.git(&["rev-parse", "refs/heads/feature"]);
    let (fixture, cx) = open_real(&repo, cx).await;
    open_editor(&fixture, "a.txt", cx).await;

    let picker = open_picker(&fixture, cx);
    wait_until(cx, "tags", |cx| {
        picker_rows(&picker, cx).contains(&"v1".to_string())
    });
    // "Local Branches > feature" is the best match.
    type_and_confirm(&picker, "feature", cx);
    let workspace = fixture.workspace.clone();
    wait_until(cx, "the revision diff", |cx| {
        workspace.read_with(cx, |workspace, cx| {
            workspace.active_item_as::<RevisionDiffView>(cx).is_some()
        })
    });
    let view = workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_item_as::<RevisionDiffView>(cx)
            .expect("view")
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.old_side(),
            &ResolvedSide::Revision {
                rev: "refs/heads/feature".into(),
                sha: branch_tip.parse().expect("sha"),
            }
        );
        assert_eq!(
            view.tab_content_text(0, cx).as_ref(),
            "a.txt (feature) \u{2194} Working Copy"
        );
    });
}

#[gpui::test]
async fn project_target_opens_a_branch_diff(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    let (fixture, cx) = open_real(&repo, cx).await;

    let picker = open_picker(&fixture, cx);
    picker.read_with(cx, |picker, _| {
        assert_eq!(
            picker.delegate.target(),
            &CompareTarget::Project {
                repo: fixture.repository.clone()
            }
        );
    });

    // A typed revision is used as it is.
    let base = repo.git(&["rev-parse", "v1"]);
    type_and_confirm(&picker, &base[..10], cx);
    wait_until(cx, "the branch diff", |cx| {
        active_branch_diff_title(&fixture, cx).is_some()
    });
    assert_eq!(
        active_branch_diff_title(&fixture, cx),
        Some(format!("Changes since {}", &base[..10]))
    );
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success(merge_base_note(&base[..10]))]
    );
    assert_eq!(
        merge_base_note("feature"),
        "Compared with the merge base of feature"
    );
    assert!(!modal_open(&fixture, cx));
}

#[gpui::test]
async fn an_invalid_revision_keeps_the_picker_open(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    let (fixture, cx) = open_real(&repo, cx).await;
    open_editor(&fixture, "a.txt", cx).await;
    let items_before = fixture
        .workspace
        .read_with(cx, |workspace, cx| workspace.items(cx).count());

    let picker = open_picker(&fixture, cx);
    for (text, message) in [
        (
            "no-such-rev",
            "'no-such-rev' is not a branch, tag or commit",
        ),
        ("-p", "refusing revision \"-p\": it starts with '-'"),
    ] {
        SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
        type_and_confirm(&picker, text, cx);
        wait_until(cx, "the error toast", |_| !shown_toasts().is_empty());
        assert_eq!(
            shown_toasts(),
            vec![OutcomeToast::error("Compare failed", message)]
        );
        assert!(modal_open(&fixture, cx), "the picker stays open");
    }
    assert_eq!(
        fixture
            .workspace
            .read_with(cx, |workspace, cx| workspace.items(cx).count()),
        items_before
    );
}

#[gpui::test]
async fn branches_popup_compare_with_current_opens_the_project_compare(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::with_two_branches();
    let (fixture, cx) = open_real(&repo, cx).await;

    let repository = fixture.repository.clone();
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        let handle = workspace.weak_handle();
        workspace.toggle_modal(window, cx, |window, cx| {
            BranchesPopup::new(handle, Some(repository), window, cx)
        });
    });
    cx.run_until_parked();
    let popup = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_modal::<BranchesPopup>(cx)
            .expect("popup is open")
    });
    let picker = popup.read_with(cx, |popup, _| popup.picker.clone());
    let row = picker.read_with(cx, |picker, _| {
        picker
            .delegate
            .rows
            .iter()
            .find_map(|row| match row {
                Row::Branch(row) if row.branch.name() == "feature" => Some(row.clone()),
                _ => None,
            })
            .expect("feature row")
    });
    let entry = crate::branches_popup::submenu_entries(&row, Some("main"))
        .into_iter()
        .find(|(action, _, _)| *action == BranchAction::CompareWithCurrent)
        .expect("compare entry");
    assert_eq!(entry.1, "Compare with 'main'");
    assert!(entry.2, "the entry is enabled");

    picker.update_in(cx, |picker, window, cx| {
        picker
            .delegate
            .run_branch_action(BranchAction::CompareWithCurrent, &row, window, cx)
    });
    wait_until(cx, "the branch diff", |cx| {
        active_branch_diff_title(&fixture, cx).is_some()
    });
    assert_eq!(
        active_branch_diff_title(&fixture, cx).as_deref(),
        Some("Changes since feature")
    );
    let tooltip = fixture.workspace.read_with(cx, |workspace, cx| {
        let diff = workspace.active_item_as::<BranchDiff>(cx)?;
        diff.read(cx)
            .tab_tooltip_text(cx)
            .map(|text| text.to_string())
    });
    assert_eq!(
        tooltip.as_deref(),
        Some("Changes since the merge base with feature")
    );
    // The merge base of `main` and `feature` is the base commit: `main`'s own change (b.txt)
    // shows, `feature`'s change to a.txt does not.
    wait_until(cx, "the branch diff content", |cx| {
        active_branch_diff_text(&fixture, cx).contains("main")
    });
    assert!(!active_branch_diff_text(&fixture, cx).contains("feature"));
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success(merge_base_note("feature"))]
    );
    let popup_open = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.active_modal::<BranchesPopup>(cx).is_some()
    });
    assert!(!popup_open, "the popup closed");
}
