//! Scale check for the Log tool window: a generated repository with 100k commits.
//!
//! Ignored by default (it takes a while and writes a ~40 MB repository to a temp dir, removed at
//! the end). Run it explicitly, preferably optimised, as the debug numbers overstate the CPU
//! work by several times:
//!
//! ```text
//! cargo test --profile release-fast -p brewery_git -- --ignored log_scale --nocapture
//! ```
//!
//! `BREWERY_LOG_SCALE_COMMITS` changes the size of the big repository (default 100000).
//!
//! For each repository it prints:
//! - raw `git log` (the Log's exact command line): time to the first 1000 lines and to the end;
//! - through the Log panel: time from showing the panel until the graph holds the first chunk,
//!   and until it is fully loaded; the main thread time spent meanwhile (in the test harness
//!   background tasks also run on this thread, so this includes reading and parsing git's
//!   output); the resident memory before and after;
//! - the cost of a frame (a full window redraw) with the Log scrolled to the top, the middle and
//!   the end, against the same on a 1000-commit repository: a frame that grows with the commit
//!   count does O(n) work.
//!
//! The big repository is measured twice: as `git fast-import` leaves it (no commit-graph file),
//! and after `git commit-graph write --reachable`.

use super::*;
use smol::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use smol::process::{Command, Stdio};
use std::io::Write as _;

const SMALL: usize = 1_000;
const FRAMES: usize = 100;

/// Writes a `git fast-import` stream: `main` plus three long-lived branches committed to in
/// turn, each merged into `main` every 40 commits, `main` merged back into `develop` every
/// 300 commits, a slow `release` branch merged every 2000 commits, and a tag every 5000.
fn fast_import_stream(commits: usize) -> Vec<u8> {
    const SIDES: [&str; 3] = ["develop", "feature-a", "feature-b"];
    let mut out = Vec::with_capacity(commits * 260);
    let mut tips: HashMap<&str, usize> = HashMap::new();
    let mut since_merge: HashMap<&str, usize> = HashMap::new();
    let mut merge_turn = 0;
    let base_time: i64 = 1_600_000_000;
    for index in 0..commits {
        let mark = index + 1;
        let branch = if index == 0 {
            "main"
        } else if index % 2000 == 1000 {
            "release"
        } else {
            match index % 8 {
                0..=3 => "main",
                4 | 5 => SIDES[0],
                6 => SIDES[1],
                _ => SIDES[2],
            }
        };
        // Every branch starts at the root commit.
        let from = tips.get(branch).copied().unwrap_or(1);
        let mut merge = None;
        if branch == "main" && index > 0 {
            if index % 2000 == 1004 {
                merge = tips.get("release").copied();
            } else if index % 40 == 0 {
                let side = SIDES[merge_turn % SIDES.len()];
                merge_turn += 1;
                if since_merge.get(side).copied().unwrap_or(0) > 0 {
                    merge = tips.get(side).copied();
                    since_merge.insert(side, 0);
                }
            }
        } else if branch == "develop" && index % 300 == 4 {
            merge = tips.get("main").copied();
        }
        let time = base_time + index as i64 * 30;
        let message = format!("Commit {index} on {branch}\n");
        let content = format!("{branch} {index}\n");
        let _ = write!(
            out,
            "commit refs/heads/{branch}\nmark :{mark}\n\
             author Scale Test <scale@brewery.dev> {time} +0000\n\
             committer Scale Test <scale@brewery.dev> {time} +0000\n\
             data {}\n{message}",
            message.len()
        );
        if index > 0 {
            let _ = writeln!(out, "from :{from}");
        }
        if let Some(merge) = merge {
            let _ = writeln!(out, "merge :{merge}");
        }
        let _ = write!(
            out,
            "M 100644 inline {branch}.txt\ndata {}\n{content}\n",
            content.len()
        );
        if index > 0 && index % 5000 == 0 {
            let _ = write!(out, "reset refs/tags/v{}\nfrom :{mark}\n\n", index / 5000);
        }
        tips.insert(branch, mark);
        *since_merge.entry(branch).or_default() += 1;
    }
    out.extend_from_slice(b"done\n");
    out
}

fn std_git(repo: &TestRepo, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.current_dir(&repo.work).envs(&repo.env).args(args);
    command
}

/// A repository with `commits` commits, checked out at `main`, with no commit-graph file.
fn generated_repo(commits: usize) -> TestRepo {
    let repo = TestRepo::new();
    let started = Instant::now();
    let stream = fast_import_stream(commits);
    smol::block_on(async {
        let mut child = std_git(&repo, &["fast-import", "--quiet", "--done"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn git fast-import");
        let mut stdin = child.stdin.take().expect("stdin");
        stdin
            .write_all(&stream)
            .await
            .expect("write the fast-import stream");
        drop(stdin);
        assert!(child.status().await.expect("fast-import").success());
    });
    repo.git(&["checkout", "--quiet", "--force", "main"]);
    let count: usize = repo
        .git(&["rev-list", "--count", "--all"])
        .parse()
        .expect("count");
    assert_eq!(count, commits);
    assert!(!has_commit_graph(&repo));
    println!(
        "generated {commits} commits in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    repo
}

/// Raw `git log` with the Log's command line (`order` is `--date-order`): (first 1000 lines,
/// all lines).
fn raw_git_log(repo: &TestRepo, order: &str) -> (Duration, Duration) {
    let started = Instant::now();
    let mut child = std_git(
        repo,
        &[
            "log",
            "--format=%H%x00%P%x00%D",
            order,
            "--ignore-missing",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--",
        ],
    )
    .stdout(Stdio::piped())
    .spawn()
    .expect("spawn git log");
    let mut reader = smol::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut first_chunk = None;
    smol::block_on(async {
        let mut line = String::new();
        let mut lines = 0;
        while reader.read_line(&mut line).await.expect("line") > 0 {
            line.clear();
            lines += 1;
            if lines == git::repository::GRAPH_CHUNK_SIZE {
                first_chunk = Some(started.elapsed());
            }
        }
        assert!(child.status().await.expect("git log").success());
    });
    let all = started.elapsed();
    (first_chunk.unwrap_or(all), all)
}

/// Zed's own loader (`RealGitRepository::initial_graph_data`: git plus parsing) on real
/// threads, outside the test scheduler: (first chunk, all chunks).
fn backend_load(repo: &TestRepo, commits: usize, cx: &TestAppContext) -> (Duration, Duration) {
    use git::repository::{GitRepository as _, RealGitRepository};
    let backend = RealGitRepository::new(
        &repo.work.join(".git"),
        None,
        Some(which::which("git").expect("git on PATH")),
        cx.executor(),
    )
    .expect("open the repository");
    let (tx, rx) = smol::channel::bounded(64);
    let started = Instant::now();
    let (first, all, received) = std::thread::scope(|scope| {
        let producer = scope.spawn(|| {
            smol::block_on(backend.initial_graph_data(LogSource::All, LogOrder::default(), tx))
        });
        let (first, all, received) = smol::block_on(async {
            let mut first = None;
            let mut received = 0;
            while let Ok(chunk) = rx.recv().await {
                received += chunk.len();
                first.get_or_insert_with(|| started.elapsed());
            }
            (first, started.elapsed(), received)
        });
        producer.join().expect("producer").expect("git log");
        (first, all, received)
    });
    assert_eq!(received, commits);
    (first.unwrap_or(all), all)
}

/// Resident memory of this process in MiB, where that is cheap to read.
fn resident_mib() -> Option<f64> {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kib: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib / 1024.)
    } else if cfg!(windows) {
        // "Image Name","PID","Session Name","Session#","Mem Usage" with Mem Usage like "123,456 K".
        let output = smol::block_on(
            Command::new("tasklist")
                .args([
                    "/FI",
                    &format!("PID eq {}", std::process::id()),
                    "/FO",
                    "CSV",
                    "/NH",
                ])
                .output(),
        )
        .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let field = text.trim().rsplit("\",\"").next()?;
        let digits: String = field.chars().filter(char::is_ascii_digit).collect();
        Some(digits.parse::<f64>().ok()? / 1024.)
    } else {
        None
    }
}

#[derive(Default)]
struct Load {
    first_chunk: Duration,
    full: Duration,
    main_thread: Duration,
    memory_before: Option<f64>,
    memory_after: Option<f64>,
}

/// Shows the Log and polls until it has the first chunk and then all `commits`.
fn load_log(fixture: &Fixture, commits: usize, cx: &mut VisualTestContext) -> Load {
    let mut load = Load {
        memory_before: resident_mib(),
        ..Load::default()
    };
    let started = Instant::now();
    cx.dispatch_action(ToggleLogPanel);
    let deadline = started + Duration::from_secs(600);
    let mut first_chunk = None;
    let mut slept = Duration::ZERO;
    let first_chunk_size = commits.min(git::repository::GRAPH_CHUNK_SIZE);
    loop {
        // One task at a time until the first chunk is in, so it is seen as soon as it lands
        // (the graph adds a chunk in the same effect flush as the repository stores it).
        while first_chunk.is_none() && cx.executor().tick() {
            if cached_commits(fixture, cx) >= first_chunk_size {
                first_chunk = Some(started.elapsed());
            }
        }
        cx.run_until_parked();
        // Read-only until everything has arrived: an update would redraw the window each time.
        if cached_commits(fixture, cx) < commits {
            assert!(Instant::now() < deadline, "timed out loading the log");
            let sleep = Instant::now();
            std::thread::sleep(Duration::from_millis(1));
            slept += sleep.elapsed();
            cx.executor().advance_clock(Duration::from_millis(16));
            continue;
        }
        let (count, loading) = log_graph(fixture, cx)
            .map(|graph| {
                graph.update(cx, |graph, cx| {
                    graph.commit_count_and_loading_state_for_test(cx)
                })
            })
            .unwrap_or((0, true));
        if first_chunk.is_none() && count >= first_chunk_size {
            first_chunk = Some(started.elapsed());
        }
        if count == commits && !loading {
            break;
        }
        assert!(Instant::now() < deadline, "timed out loading the log");
        let sleep = Instant::now();
        std::thread::sleep(Duration::from_millis(1));
        slept += sleep.elapsed();
        cx.executor().advance_clock(Duration::from_millis(16));
    }
    load.full = started.elapsed();
    load.main_thread = load.full.saturating_sub(slept);
    load.first_chunk = first_chunk.unwrap_or(load.full);
    load.memory_after = resident_mib();
    load
}

/// The commits the repository has received for the unfiltered log so far.
fn cached_commits(fixture: &Fixture, cx: &VisualTestContext) -> usize {
    repository(fixture, cx).read_with(cx, |repo, _| {
        repo.get_graph_data(LogSource::All, LogOrder::default())
            .map_or(0, |data| data.commit_data.len())
    })
}

/// Median of `FRAMES` runs of `frame`.
fn median(mut frame: impl FnMut() -> Duration) -> Duration {
    let mut times = (0..FRAMES).map(|_| frame()).collect::<Vec<_>>();
    times.sort();
    times[FRAMES / 2]
}

struct Frames {
    at: &'static str,
    /// A full window redraw (the workspace around the panel included).
    window: Duration,
    /// Layout and paint of the graph view alone, 1600x600.
    graph: Duration,
}

/// Median costs of a full window redraw and of drawing the graph view alone.
fn frame_cost(at: &'static str, graph: &Entity<GitGraph>, cx: &mut VisualTestContext) -> Frames {
    let window = median(|| {
        let started = Instant::now();
        cx.update(|window, _| window.refresh());
        started.elapsed()
    });
    let graph_only = median(|| {
        let started = Instant::now();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1600.), px(600.)),
            |_, _| graph.clone().into_any_element(),
        );
        started.elapsed()
    });
    Frames {
        at,
        window,
        graph: graph_only,
    }
}

/// Frame costs with the Log scrolled to the top, the middle and the end (by selecting the
/// commit there, which scrolls it into view).
fn frames_at_top_middle_end(fixture: &Fixture, cx: &mut VisualTestContext) -> Vec<Frames> {
    let graph = log_graph(fixture, cx).expect("graph");
    let shas = graph.read_with(cx, |graph, _| {
        graph
            .initial_commit_data_for_test()
            .iter()
            .map(|commit| commit.sha)
            .collect::<Vec<_>>()
    });
    let mut result = Vec::new();
    for (name, index) in [
        ("top", 0),
        ("middle", shas.len() / 2),
        ("end", shas.len() - 1),
    ] {
        graph.update(cx, |graph, cx| graph.select_commit_by_sha(shas[index], cx));
        // Let the selection's commit message and diff load (real git processes).
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
            cx.executor().advance_clock(Duration::from_millis(16));
        }
        assert_eq!(
            graph.read_with(cx, |graph, _| graph.selected_commit_sha()),
            Some(shas[index])
        );
        result.push(frame_cost(name, &graph, cx));
    }
    result
}

/// Time to build a second `GitGraph` once the repository has every commit cached: its
/// constructor runs `GraphData::add_commits` over all of them at once.
fn lane_assignment(fixture: &Fixture, commits: usize, cx: &mut VisualTestContext) -> Duration {
    let started = Instant::now();
    let graph = fixture.workspace.update_in(cx, |workspace, window, cx| {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let repo_id = project
            .read(cx)
            .active_repository(cx)
            .expect("repository")
            .read(cx)
            .id;
        let weak = workspace.weak_handle();
        cx.new(|cx| GitGraph::new(repo_id, git_store, weak, None, window, cx))
    });
    let elapsed = started.elapsed();
    let state = graph.update(cx, |graph, cx| {
        graph.commit_count_and_loading_state_for_test(cx)
    });
    assert_eq!(state, (commits, false));
    elapsed
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.)
}

fn mib(memory: Option<f64>) -> String {
    memory.map_or_else(|| "n/a".to_string(), |mib| format!("{mib:.0} MiB"))
}

async fn measure(label: &str, repo: &TestRepo, commits: usize, cx: &mut TestAppContext) {
    let (raw_first, raw_all) = raw_git_log(repo, "--date-order");
    let (topo_first, topo_all) = raw_git_log(repo, "--topo-order");
    let (backend_first, backend_all) = backend_load(repo, commits, cx);
    let (fixture, cx) = open_real(repo, cx).await;
    cx.simulate_resize(gpui::size(px(1600.), px(1000.)));
    let load = load_log(&fixture, commits, cx);
    let frames = frames_at_top_middle_end(&fixture, cx);
    println!("== {label}: {commits} commits");
    println!(
        "   raw git log: first {} lines {}, all {}",
        git::repository::GRAPH_CHUNK_SIZE,
        ms(raw_first),
        ms(raw_all)
    );
    println!(
        "   raw git log --topo-order instead: first {} lines {}, all {}",
        git::repository::GRAPH_CHUNK_SIZE,
        ms(topo_first),
        ms(topo_all)
    );
    println!(
        "   Zed's loader (git + parsing, real threads): first chunk {}, all {}",
        ms(backend_first),
        ms(backend_all)
    );
    println!(
        "   Log panel: first chunk {}, full load {}, test thread busy {}, memory {} -> {}",
        ms(load.first_chunk),
        ms(load.full),
        ms(load.main_thread),
        mib(load.memory_before),
        mib(load.memory_after)
    );
    for frames in frames {
        println!(
            "   frame at {}: window {}, graph alone {}",
            frames.at,
            ms(frames.window),
            ms(frames.graph)
        );
    }
    let notice = fixture
        .panel
        .read_with(cx, |panel, _| panel.commit_graph_notice().cloned());
    println!("   commit-graph notice: {notice:?}");
    println!(
        "   lane assignment (a new GitGraph over the cached {commits} commits): {}",
        ms(lane_assignment(&fixture, commits, cx))
    );
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
}

#[gpui::test]
#[ignore = "scale check: run explicitly with --ignored log_scale"]
async fn log_scale_100k_commits(cx: &mut TestAppContext) {
    init_test(cx);
    let big = std::env::var("BREWERY_LOG_SCALE_COMMITS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    println!(
        "profile: {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "optimised"
        }
    );

    let small_repo = generated_repo(SMALL);
    measure("small, no commit-graph", &small_repo, SMALL, cx).await;
    drop(small_repo);

    let big_repo = generated_repo(big);
    measure("big, no commit-graph", &big_repo, big, cx).await;

    let started = Instant::now();
    big_repo.git(&["commit-graph", "write", "--reachable"]);
    println!(
        "git commit-graph write --reachable: {}",
        ms(started.elapsed())
    );
    assert!(has_commit_graph(&big_repo));
    measure("big, with commit-graph", &big_repo, big, cx).await;
}
