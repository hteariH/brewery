use super::*;
use fs::RealFs;
use git::repository::{CommitSummary, RepoPath};
use gpui::{TestAppContext, VisualTestContext};
use project::{FakeFs, Project};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use util::path;
use workspace::MultiWorkspace;
use worktree::WorktreeModelHandle;

fn branch(name: &str) -> Branch {
    let ref_name = if name.starts_with("refs/") {
        name.to_string()
    } else if let Some(remote) = name.strip_prefix("remote:") {
        format!("refs/remotes/{remote}")
    } else {
        format!("refs/heads/{name}")
    };
    Branch {
        is_head: false,
        ref_name: ref_name.into(),
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

fn head(name: &str) -> Branch {
    Branch {
        is_head: true,
        ..branch(name)
    }
}

/// One line per row: `> action`, `# header`, `* current`, `  branch [group]`.
fn describe(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Action { action, .. } => format!("> {}", action.label()),
            Row::Header(header) => format!("# {}", header.title()),
            Row::Branch(row) => format!(
                "{} {} [{:?}]",
                if row.is_current { "*" } else { " " },
                row.branch.name(),
                row.group
            ),
            Row::Input(input) => format!("? {}", input.label()),
        })
        .collect()
}

fn sample_branches() -> Vec<Branch> {
    vec![
        head("main"),
        branch("feature-b"),
        branch("feature-a"),
        branch("develop"),
        branch("remote:origin/main"),
        branch("remote:origin/HEAD"),
        branch("remote:origin/feature-a"),
        branch("remote:upstream/release"),
    ]
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn groups_actions_recent_local_and_remotes_by_remote() {
    let grouped = group_rows(
        &sample_branches(),
        Some("main"),
        &strings(&["main", "feature-b", "gone", "develop"]),
        "",
    );
    assert_eq!(
        describe(&grouped.rows),
        vec![
            "> New Branch…",
            "> Checkout Tag or Revision…",
            "> Update Project",
            "> Push…",
            "# Recent",
            "  feature-b [Recent]",
            "  develop [Recent]",
            "# Local",
            "  develop [Local]",
            "  feature-a [Local]",
            "  feature-b [Local]",
            "* main [Local]",
            "# Remote",
            "# origin",
            "  origin/feature-a [Remote]",
            "  origin/main [Remote]",
            "# upstream",
            "  upstream/release [Remote]",
        ]
    );
    // The first branch row is selected by default.
    assert_eq!(grouped.selected, Some(5));
}

#[test]
fn recent_excludes_current_and_keeps_at_most_five() {
    let branches = ["a", "b", "c", "d", "e", "f", "g"]
        .into_iter()
        .map(branch)
        .collect::<Vec<_>>();
    let recent = strings(&["c", "a", "b", "d", "e", "f", "g"]);
    let grouped = group_rows(&branches, Some("c"), &recent, "");
    let recent_rows = grouped
        .rows
        .iter()
        .filter_map(Row::as_branch)
        .filter(|row| row.group == Group::Recent)
        .map(|row| row.branch.name().to_string())
        .collect::<Vec<_>>();
    assert_eq!(recent_rows, strings(&["a", "b", "d", "e", "f"]));

    // Without recent branches there is no Recent header.
    let grouped = group_rows(&branches, Some("c"), &strings(&["c"]), "");
    assert!(!describe(&grouped.rows).contains(&"# Recent".to_string()));
}

#[test]
fn typing_filters_every_group_and_selects_the_best_match() {
    let grouped = group_rows(
        &sample_branches(),
        Some("main"),
        &strings(&["feature-a", "develop"]),
        "feata",
    );
    assert_eq!(
        describe(&grouped.rows),
        vec![
            "# Recent",
            "  feature-a [Recent]",
            "# Local",
            "  feature-a [Local]",
            "# Remote",
            "# origin",
            "  origin/feature-a [Remote]",
        ]
    );
    assert!(
        grouped
            .selected
            .is_some_and(|ix| grouped.rows[ix].as_branch().is_some())
    );
    let positions = &grouped.rows[1].as_branch().map(|row| row.positions.clone());
    assert!(positions.as_ref().is_some_and(|p| !p.is_empty()));

    // Actions are filtered too; an empty group loses its header.
    let grouped = group_rows(&sample_branches(), Some("main"), &[], "push");
    assert_eq!(describe(&grouped.rows), vec!["> Push…"]);
    assert_eq!(grouped.selected, Some(0));

    let grouped = group_rows(&sample_branches(), Some("main"), &[], "release");
    assert_eq!(
        describe(&grouped.rows),
        vec!["# Remote", "# upstream", "  upstream/release [Remote]"]
    );
    assert_eq!(grouped.selected, Some(2));

    let grouped = group_rows(&sample_branches(), Some("main"), &[], "zzzz");
    assert!(grouped.rows.is_empty());
    assert_eq!(grouped.selected, None);
}

#[test]
fn detached_head_marks_nothing_current() {
    let grouped = group_rows(&sample_branches(), None, &strings(&["main"]), "");
    assert!(
        grouped
            .rows
            .iter()
            .filter_map(Row::as_branch)
            .all(|row| !row.is_current)
    );
    assert!(describe(&grouped.rows).contains(&"  main [Recent]".to_string()));
}

#[test]
fn submenu_entries_depend_on_branch_kind() {
    let rows = group_rows(&sample_branches(), Some("main"), &[], "").rows;
    let find = |name: &str| {
        rows.iter()
            .filter_map(Row::as_branch)
            .find(|row| row.branch.name() == name && row.group != Group::Recent)
            .cloned()
            .expect("row")
    };
    let labels = |row: &BranchRow, current: Option<&str>| {
        submenu_entries(row, current)
            .into_iter()
            .map(|(_, label, enabled)| format!("{}{label}", if enabled { "" } else { "(off) " }))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(&find("feature-a"), Some("main")),
        vec![
            "Checkout",
            "New Branch from 'feature-a'…",
            "Checkout and Rebase onto 'main'",
            "Rebase 'main' onto 'feature-a'",
            "Merge 'feature-a' into 'main'",
            "(off) Compare with 'main'",
            "Rename…",
            "Delete",
        ]
    );
    assert_eq!(
        labels(&find("main"), Some("main")),
        vec!["New Branch from 'main'…", "Rename…"]
    );
    assert_eq!(
        labels(&find("origin/main"), Some("main")),
        vec![
            "Checkout",
            "New Branch from 'origin/main'…",
            "Checkout and Rebase onto 'main'",
            "Rebase 'main' onto 'origin/main'",
            "Merge 'origin/main' into 'main'",
            "(off) Compare with 'main'",
        ]
    );
    // Detached HEAD: nothing to merge into or rebase.
    assert_eq!(
        labels(&find("feature-a"), None),
        vec![
            "Checkout",
            "New Branch from 'feature-a'…",
            "Rename…",
            "Delete"
        ]
    );
}

#[test]
fn merge_and_rebase_outcomes_become_toasts() {
    let merge = GitOp::Merge {
        branch: "feature".into(),
        into: "main".into(),
    };
    let rebase = GitOp::Rebase {
        branch: "main".into(),
        onto: "feature".into(),
    };
    let paths = |names: &[&str]| {
        names
            .iter()
            .map(|name| RepoPath::new(name).expect("path"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        outcome_toast(&merge, &Ok(MergeOutcome::Merged)),
        OutcomeToast::success("Merged 'feature' into 'main'")
    );
    assert_eq!(
        outcome_toast(&rebase, &Ok(MergeOutcome::Merged)),
        OutcomeToast::success("Rebased 'main' onto 'feature'")
    );
    assert_eq!(
        outcome_toast(&merge, &Ok(MergeOutcome::UpToDate)).kind,
        ToastKind::Success
    );
    assert_eq!(
        outcome_toast(
            &merge,
            &Ok(MergeOutcome::Conflicts(paths(&["a.txt", "b.txt"])))
        ),
        OutcomeToast {
            kind: ToastKind::Conflicts,
            message: "Merge has conflicts: 2 files".into()
        }
    );
    assert_eq!(
        outcome_toast(&rebase, &Ok(MergeOutcome::Conflicts(paths(&["a.txt"])))).message,
        "Rebase has conflicts: 1 file"
    );
    assert_eq!(
        outcome_toast(
            &merge,
            &Ok(MergeOutcome::Failed(
                "\nerror: Your local changes would be overwritten\nAborting".into()
            ))
        ),
        OutcomeToast {
            kind: ToastKind::Error,
            message: "Merge failed: error: Your local changes would be overwritten".into()
        }
    );
    assert_eq!(
        outcome_toast(&rebase, &Err(anyhow!("not supported for remote projects"))),
        OutcomeToast {
            kind: ToastKind::Error,
            message: "Rebase failed: not supported for remote projects".into()
        }
    );
}

// ---------------------------------------------------------------------------------------------
// gpui

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = settings::SettingsStore::test(cx);
        cx.set_global(settings_store);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
    });
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
}

fn shown_toasts() -> Vec<OutcomeToast> {
    SHOWN_TOASTS.with(|shown| shown.borrow().clone())
}

struct Opened {
    workspace: Entity<Workspace>,
    repository: Entity<Repository>,
    cx: VisualTestContext,
}

async fn open_workspace(project: Entity<Project>, cx: &mut TestAppContext) -> Opened {
    let repository = project.read_with(cx, |project, cx| {
        project.active_repository(cx).expect("repository")
    });
    let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = window
        .read_with(cx, |multi, _| multi.workspace().clone())
        .expect("workspace");
    let cx = VisualTestContext::from_window(window.into(), cx);
    Opened {
        workspace,
        repository,
        cx,
    }
}

fn open_popup(opened: &mut Opened) -> Entity<BranchesPopup> {
    let repository = opened.repository.clone();
    opened
        .workspace
        .update_in(&mut opened.cx, |workspace, window, cx| {
            let handle = workspace.weak_handle();
            workspace.toggle_modal(window, cx, |window, cx| {
                BranchesPopup::new(handle, Some(repository), window, cx)
            });
        });
    opened.cx.run_until_parked();
    opened.workspace.read_with(&opened.cx, |workspace, cx| {
        workspace
            .active_modal::<BranchesPopup>(cx)
            .expect("popup is open")
    })
}

fn rows(popup: &Entity<BranchesPopup>, cx: &mut VisualTestContext) -> Vec<String> {
    popup.read_with(cx, |popup, cx| {
        describe(&popup.picker.read(cx).delegate.rows)
    })
}

fn branch_row(popup: &Entity<BranchesPopup>, name: &str, cx: &mut VisualTestContext) -> BranchRow {
    popup.read_with(cx, |popup, cx| {
        popup
            .picker
            .read(cx)
            .delegate
            .rows
            .iter()
            .filter_map(Row::as_branch)
            .find(|row| row.branch.name() == name && row.group != Group::Recent)
            .cloned()
            .expect("branch row")
    })
}

fn run_action(
    popup: &Entity<BranchesPopup>,
    action: BranchAction,
    name: &str,
    cx: &mut VisualTestContext,
) {
    let row = branch_row(popup, name, cx);
    let picker = popup.read_with(cx, |popup, _| popup.picker.clone());
    picker.update_in(cx, |picker, window, cx| {
        picker.delegate.run_branch_action(action, &row, window, cx)
    });
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
        // The fs watcher debounces with the executor's (fake) clock.
        cx.executor().advance_clock(Duration::from_millis(200));
    }
}

#[gpui::test]
async fn popup_lists_groups_for_a_fake_branch_list(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(path!("/dir"), json!({ ".git": {}, "file.txt": "text" }))
        .await;
    let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
    cx.run_until_parked();
    let mut opened = open_workspace(project, cx).await;
    opened.repository.update(&mut opened.cx, |repository, cx| {
        repository.set_branch_list_for_test(sample_branches(), cx)
    });
    let popup = open_popup(&mut opened);
    let cx = &mut opened.cx;

    // The fake repository has no reflog: inject the recent list.
    let picker = popup.read_with(cx, |popup, _| popup.picker.clone());
    picker.update_in(cx, |picker, window, cx| {
        picker.delegate.recent = strings(&["develop"]);
        picker.refresh(window, cx);
    });
    cx.run_until_parked();
    let listed = rows(&popup, cx);
    assert_eq!(
        &listed[..6],
        &[
            "> New Branch…",
            "> Checkout Tag or Revision…",
            "> Update Project",
            "> Push…",
            "# Recent",
            "  develop [Recent]",
        ]
    );
    assert!(listed.contains(&"# Local".to_string()));
    assert!(listed.contains(&"# origin".to_string()));
    assert!(listed.contains(&"# upstream".to_string()));
    assert!(!listed.iter().any(|row| row.contains("origin/HEAD")));

    // Typing filters.
    picker.update_in(cx, |picker, window, cx| {
        picker.set_query("release", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        rows(&popup, cx),
        vec!["# Remote", "# upstream", "  upstream/release [Remote]"]
    );

    // A branch list change refreshes the open popup.
    picker.update_in(cx, |picker, window, cx| picker.set_query("", window, cx));
    opened.repository.update(cx, |repository, cx| {
        repository.set_branch_list_for_test(vec![head("main"), branch("hotfix")], cx)
    });
    cx.run_until_parked();
    assert!(rows(&popup, cx).contains(&"  hotfix [Local]".to_string()));

    // → opens the submenu for the selected branch.
    let ix = popup.read_with(cx, |popup, cx| {
        popup
            .picker
            .read(cx)
            .delegate
            .rows
            .iter()
            .position(|row| row.as_branch().is_some_and(|b| b.branch.name() == "hotfix"))
            .expect("hotfix row")
    });
    picker.update_in(cx, |picker, window, cx| {
        picker.set_selected_index(ix, None, true, window, cx)
    });
    cx.dispatch_action(editor::actions::MoveRight);
    cx.run_until_parked();
    let menu = popup
        .read_with(cx, |popup, cx| {
            popup
                .picker
                .read(cx)
                .delegate
                .submenu
                .as_ref()
                .map(|submenu| submenu.menu.clone())
        })
        .expect("submenu is open");
    // (The menu takes focus two frames later, like Zed's popover menus; test windows do not
    // run frame callbacks.) Closing it returns to the list; the popup stays open.
    menu.update(cx, |_, cx| cx.emit(DismissEvent));
    cx.run_until_parked();
    assert!(popup.read_with(cx, |popup, cx| {
        popup.picker.read(cx).delegate.submenu.is_none()
    }));
    assert!(picker.update_in(cx, |picker, window, cx| {
        picker.focus_handle(cx).is_focused(window)
    }));
    opened.workspace.read_with(cx, |workspace, cx| {
        assert!(workspace.active_modal::<BranchesPopup>(cx).is_some())
    });
}

// Real repositories ----------------------------------------------------------------------------

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
            ("core.safecrlf", "false"),
            ("user.name", "Brewery Test"),
            ("user.email", "test@brewery.dev"),
            ("commit.gpgsign", "false"),
            ("merge.ff", "true"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    fn git_status(&self, args: &[&str]) -> (bool, String) {
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .env("GIT_EDITOR", "true")
                .args(args)
                .output(),
        )
        .expect("spawn git");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )
    }

    fn git(&self, args: &[&str]) -> String {
        let (ok, stdout) = self.git_status(args);
        assert!(ok, "git {args:?} failed");
        stdout
    }

    fn commit(&self, path: &str, content: &str, message: &str) {
        std::fs::write(self.work.join(path), content).expect("write");
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn has_branch(&self, name: &str) -> bool {
        self.git_status(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .0
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }
}

async fn open_real(repo: &TestRepo, cx: &mut TestAppContext) -> Opened {
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
    open_workspace(project, cx).await
}

fn wait_for_branch_list(opened: &mut Opened, what: &str, check: impl Fn(&[String]) -> bool) {
    let repository = opened.repository.clone();
    wait_until(&mut opened.cx, what, |cx| {
        let names = repository.read_with(cx, |repository, _| {
            repository
                .branch_list
                .iter()
                .map(|b| b.name().to_string())
                .collect::<Vec<_>>()
        });
        check(&names)
    });
}

fn has_notification(opened: &mut Opened, kind: ToastKind) -> bool {
    opened.workspace.read_with(&opened.cx, |workspace, _| {
        workspace.has_notification(&toast_id(kind))
    })
}

#[gpui::test]
async fn merge_into_current_reports_merged_and_refreshes(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.commit("a.txt", "feature\n", "feature change");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.commit("b.txt", "main\n", "main change");
    let mut opened = open_real(&repo, cx).await;
    wait_for_branch_list(&mut opened, "branches", |names| names.len() == 2);

    let popup = open_popup(&mut opened);
    // The reflog makes `feature` recent; `main` (current) is excluded.
    wait_until(&mut opened.cx, "recent group", |cx| {
        rows(&popup, cx).contains(&"  feature [Recent]".to_string())
    });
    let head_before = opened.repository.read_with(&opened.cx, |repository, _| {
        repository.head_commit.as_ref().map(|c| c.sha.clone())
    });
    run_action(
        &popup,
        BranchAction::MergeIntoCurrent,
        "feature",
        &mut opened.cx,
    );
    wait_until(&mut opened.cx, "merge toast", |_| {
        !shown_toasts().is_empty()
    });
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success("Merged 'feature' into 'main'")]
    );
    assert!(has_notification(&mut opened, ToastKind::Success));
    assert_eq!(
        repo.git(&["show", "-s", "--format=%P", "HEAD"])
            .split(' ')
            .count(),
        2
    );

    // Zed's repository watcher sees the merge commit without any rescan from us.
    let repository = opened.repository.clone();
    wait_until(&mut opened.cx, "head commit refresh", |cx| {
        repository.read_with(cx, |repository, _| {
            repository.head_commit.as_ref().map(|c| c.sha.clone()) != head_before
        })
    });
    let head_after = repo.head();
    assert_eq!(
        opened
            .repository
            .read_with(&opened.cx, |repository, _| repository
                .head_commit
                .as_ref()
                .map(|c| c.sha.to_string())),
        Some(head_after)
    );
}

#[gpui::test]
async fn conflicting_merge_reports_conflicts(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.commit("c.txt", "base\n", "base c");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    std::fs::write(repo.work.join("a.txt"), "feature\n").expect("write");
    repo.commit("c.txt", "feature\n", "feature change");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "--quiet", "--amend", "--no-edit"]);
    repo.git(&["checkout", "--quiet", "main"]);
    std::fs::write(repo.work.join("a.txt"), "main\n").expect("write");
    repo.commit("c.txt", "main\n", "main change");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "--quiet", "--amend", "--no-edit"]);
    let mut opened = open_real(&repo, cx).await;
    wait_for_branch_list(&mut opened, "branches", |names| names.len() == 2);

    let popup = open_popup(&mut opened);
    run_action(
        &popup,
        BranchAction::MergeIntoCurrent,
        "feature",
        &mut opened.cx,
    );
    wait_until(&mut opened.cx, "merge toast", |_| {
        !shown_toasts().is_empty()
    });
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast {
            kind: ToastKind::Conflicts,
            message: "Merge has conflicts: 2 files".into()
        }]
    );
    assert!(has_notification(&mut opened, ToastKind::Conflicts));
    // The popup closed when the merge started.
    opened.workspace.read_with(&opened.cx, |workspace, cx| {
        assert!(workspace.active_modal::<BranchesPopup>(cx).is_none())
    });
    assert!(
        repo.git_status(&["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
            .0
    );
}

#[gpui::test]
async fn delete_asks_first_and_offers_force_delete(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["branch", "merged"]);
    repo.git(&["checkout", "--quiet", "-b", "unmerged"]);
    repo.commit("u.txt", "unmerged\n", "unmerged work");
    repo.git(&["checkout", "--quiet", "main"]);
    let mut opened = open_real(&repo, cx).await;
    wait_for_branch_list(&mut opened, "branches", |names| names.len() == 3);

    // Cancel keeps the branch.
    let popup = open_popup(&mut opened);
    run_action(&popup, BranchAction::Delete, "merged", &mut opened.cx);
    wait_until(&mut opened.cx, "delete prompt", |cx| {
        cx.has_pending_prompt()
    });
    opened.cx.simulate_prompt_answer("Cancel");
    opened.cx.run_until_parked();
    assert!(repo.has_branch("merged"));
    assert!(shown_toasts().is_empty());

    // Delete removes a merged branch.
    let popup = open_popup(&mut opened);
    run_action(&popup, BranchAction::Delete, "merged", &mut opened.cx);
    wait_until(&mut opened.cx, "delete prompt", |cx| {
        cx.has_pending_prompt()
    });
    opened.cx.simulate_prompt_answer("Delete");
    wait_until(&mut opened.cx, "delete toast", |_| {
        !shown_toasts().is_empty()
    });
    assert!(!repo.has_branch("merged"));
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success("Deleted branch 'merged'")]
    );
    wait_for_branch_list(&mut opened, "branch list without merged", |names| {
        !names.iter().any(|name| name == "merged")
    });

    // An unmerged branch needs a second confirmation.
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
    let popup = open_popup(&mut opened);
    run_action(&popup, BranchAction::Delete, "unmerged", &mut opened.cx);
    wait_until(&mut opened.cx, "delete prompt", |cx| {
        cx.has_pending_prompt()
    });
    opened.cx.simulate_prompt_answer("Delete");
    wait_until(&mut opened.cx, "force delete prompt", |cx| {
        cx.has_pending_prompt()
    });
    assert!(repo.has_branch("unmerged"));
    opened.cx.simulate_prompt_answer("Force Delete");
    wait_until(&mut opened.cx, "delete toast", |_| {
        !shown_toasts().is_empty()
    });
    assert!(!repo.has_branch("unmerged"));
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success("Deleted branch 'unmerged'")]
    );
}

#[gpui::test]
async fn rename_new_branch_and_checkout_revision(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["tag", "v1"]);
    repo.commit("a.txt", "next\n", "next");
    repo.git(&["branch", "old-name"]);
    let mut opened = open_real(&repo, cx).await;
    wait_for_branch_list(&mut opened, "branches", |names| names.len() == 2);

    // Rename… switches the search field to a name input prefilled with the old name.
    let popup = open_popup(&mut opened);
    run_action(&popup, BranchAction::Rename, "old-name", &mut opened.cx);
    opened.cx.run_until_parked();
    let picker = popup.read_with(&opened.cx, |popup, _| popup.picker.clone());
    assert_eq!(
        picker.read_with(&opened.cx, |picker, cx| picker.query(cx)),
        "old-name"
    );
    assert_eq!(
        rows(&popup, &mut opened.cx),
        vec!["? Type a new name for 'old-name'"]
    );
    picker.update_in(&mut opened.cx, |picker, window, cx| {
        picker.set_query("new name", window, cx)
    });
    opened.cx.run_until_parked();
    assert_eq!(
        rows(&popup, &mut opened.cx),
        vec!["? Rename 'old-name' to 'new-name'"]
    );
    opened.cx.dispatch_action(menu::Confirm);
    wait_until(&mut opened.cx, "rename toast", |_| {
        !shown_toasts().is_empty()
    });
    assert_eq!(
        shown_toasts(),
        vec![OutcomeToast::success("Renamed 'old-name' to 'new-name'")]
    );
    assert!(repo.has_branch("new-name") && !repo.has_branch("old-name"));
    wait_for_branch_list(&mut opened, "renamed branch", |names| {
        names.iter().any(|name| name == "new-name")
    });

    // Escape leaves an input mode without closing the popup.
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
    let popup = open_popup(&mut opened);
    let picker = popup.read_with(&opened.cx, |popup, _| popup.picker.clone());
    picker.update_in(&mut opened.cx, |picker, window, cx| {
        picker
            .delegate
            .run_top_action(TopAction::NewBranch, window, cx)
    });
    opened.cx.run_until_parked();
    assert_eq!(
        rows(&popup, &mut opened.cx),
        vec!["? Type a name for the new branch"]
    );
    opened.cx.dispatch_action(menu::Cancel);
    opened.cx.run_until_parked();
    assert!(rows(&popup, &mut opened.cx).contains(&"# Local".to_string()));

    // New Branch from 'main'… creates and checks out the branch.
    run_action(&popup, BranchAction::NewBranchFrom, "main", &mut opened.cx);
    opened.cx.run_until_parked();
    picker.update_in(&mut opened.cx, |picker, window, cx| {
        picker.set_query("topic", window, cx)
    });
    opened.cx.run_until_parked();
    assert_eq!(
        rows(&popup, &mut opened.cx),
        vec!["? Create branch 'topic' from 'main'"]
    );
    opened.cx.dispatch_action(menu::Confirm);
    wait_until(&mut opened.cx, "create toast", |_| {
        !shown_toasts().is_empty()
    });
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "topic");

    // Checkout Tag or Revision… detaches HEAD at the tag; an unknown revision is refused.
    for (rev, expected) in [
        (
            "no-such-rev",
            OutcomeToast::error(
                "Checkout failed",
                "'no-such-rev' is not a tag, branch or commit",
            ),
        ),
        (
            "--orphan",
            OutcomeToast::error(
                "git failed",
                "refusing revision \"--orphan\": it starts with '-'",
            ),
        ),
        (
            "v1",
            OutcomeToast::success("Checked out 'v1' (detached HEAD)"),
        ),
    ] {
        SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
        let popup = open_popup(&mut opened);
        let picker = popup.read_with(&opened.cx, |popup, _| popup.picker.clone());
        picker.update_in(&mut opened.cx, |picker, window, cx| {
            picker
                .delegate
                .run_top_action(TopAction::CheckoutRevision, window, cx)
        });
        opened.cx.run_until_parked();
        picker.update_in(&mut opened.cx, |picker, window, cx| {
            picker.set_query(rev, window, cx)
        });
        opened.cx.run_until_parked();
        opened.cx.dispatch_action(menu::Confirm);
        wait_until(&mut opened.cx, "checkout toast", |_| {
            !shown_toasts().is_empty()
        });
        assert_eq!(shown_toasts(), vec![expected]);
    }
    assert_eq!(repo.head(), repo.git(&["rev-parse", "v1^{commit}"]));
    assert!(!repo.git_status(&["symbolic-ref", "--quiet", "HEAD"]).0);
}
