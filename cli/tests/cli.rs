//! End-to-end tests: the real binary against fixture config dirs in a tempdir
//! (`CLAUDE_CONFIG_DIR`). Nothing here touches `~/.claude`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

// The `--project` argument and its slug follow the platform's path shape; the
// fixture transcripts record `CWD` either way.
#[cfg(not(windows))]
const PROJECT: &str = "/work/proj";
#[cfg(not(windows))]
const SLUG: &str = "-work-proj";
#[cfg(not(windows))]
const OTHER: &str = "/other/proj";
#[cfg(not(windows))]
const OTHER_SLUG: &str = "-other-proj";
#[cfg(windows)]
const PROJECT: &str = r"C:\work\proj";
#[cfg(windows)]
const SLUG: &str = "C--work-proj";
#[cfg(windows)]
const OTHER: &str = r"C:\other\proj";
#[cfg(windows)]
const OTHER_SLUG: &str = "C--other-proj";
const CWD: &str = "/work/proj";

const A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const C: &str = "cccccccc-0000-4000-8000-000000000003";
const D: &str = "dddddddd-0000-4000-8000-000000000004";
const E: &str = "eeeeeeee-0000-4000-8000-000000000005";

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn session(&self, id: &str, lines: &[String]) {
        let mut body = lines.join("\n");
        body.push('\n');
        self.write(&format!("projects/{SLUG}/{id}.jsonl"), &body);
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sessile"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", self.root())
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .output()
            .unwrap()
    }

    /// Runs `run <args> --json`, expects exit 0, returns stdout as JSON.
    fn ok(&self, args: &[&str]) -> Value {
        let mut full = args.to_vec();
        full.push("--json");
        let out = self.run(&full);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Runs `run <args> --json`, expects the given exit code and nothing on
    /// stdout, returns the error object from the last stderr line.
    fn fails(&self, args: &[&str], code: i32) -> Value {
        let mut full = args.to_vec();
        full.push("--json");
        let out = self.run(&full);
        assert_eq!(out.status.code(), Some(code), "{args:?}: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "{args:?} printed a result on failure"
        );
        last_line_json(&out.stderr)["error"].clone()
    }

    /// The error `kind` of a call that must fail with exit 1.
    fn kind(&self, args: &[&str]) -> String {
        self.fails(args, 1)["kind"].as_str().unwrap().to_string()
    }

    fn items(&self, args: &[&str]) -> Vec<Value> {
        self.ok(args)["items"].as_array().unwrap().clone()
    }

    fn list(&self) -> Vec<Value> {
        self.items(&["list", "--project", PROJECT])
    }

    fn row(&self, id: &str) -> Value {
        self.list()
            .into_iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id} not listed"))
    }

    /// Every file under the config dir: relative path to content.
    fn snapshot(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        walk(self.root(), self.root(), &mut map);
        map
    }

    /// A session with a file for every part of the footprint.
    fn full_footprint(&self, id: &str) -> Vec<String> {
        let rels = [
            format!("projects/{SLUG}/{id}/subagents/agent-1.jsonl"),
            format!("projects/{SLUG}/{id}/tool-results/r.txt"),
            format!("session-env/{id}/env"),
            format!("file-history/{id}/h@v1"),
            format!("tasks/{id}/1.json"),
            format!("debug/{id}.txt"),
            format!("todos/{id}-agent-{id}.json"),
            format!("security/x-{id}-y.json"),
            format!("telemetry/1p_failed_events.{id}.z.json"),
        ];
        for rel in &rels {
            self.write(rel, "data");
        }
        rels.to_vec()
    }

    /// Registers a session file for `pid`, started at `proc_start` (`None` omits it).
    fn live(&self, id: &str, pid: u32, proc_start: Option<&str>) {
        let mut v = json!({"pid": pid, "sessionId": id, "cwd": PROJECT});
        if let Some(s) = proc_start {
            v["procStart"] = json!(s);
        }
        self.write(&format!("sessions/{pid}.json"), &v.to_string());
    }
}

fn last_line_json(stderr: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stderr {text:?} has no JSON last line: {e}"))
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            out.insert(slashed(&rel), fs::read_to_string(&p).unwrap());
        }
    }
}

/// A path as the tests spell it: `/` on every platform.
fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

fn user(text: &str) -> String {
    format!(
        r#"{{"parentUuid":null,"type":"user","message":{{"role":"user","content":{}}},"cwd":"/work/proj","gitBranch":"main","version":"2.1.0"}}"#,
        json!(text)
    )
}

fn user_blocks(texts: &[&str]) -> String {
    let blocks: Vec<Value> = texts
        .iter()
        .map(|t| json!({"type": "text", "text": t}))
        .collect();
    format!(
        r#"{{"type":"user","message":{{"role":"user","content":{}}}}}"#,
        json!(blocks)
    )
}

fn tool_result() -> String {
    r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"ok"}]},"toolUseResult":{"x":1}}"#.into()
}

fn assistant(text: &str, model: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","model":"{model}","content":[{{"type":"text","text":{}}}]}},"cwd":"/work/proj","gitBranch":"feat/x","version":"2.1.1"}}"#,
        json!(text)
    )
}

fn custom_title(id: &str, title: &str) -> String {
    format!(
        r#"{{"type":"custom-title","customTitle":{},"sessionId":"{id}"}}"#,
        json!(title)
    )
}

fn ai_title(id: &str, title: &str) -> String {
    format!(
        r#"{{"type":"ai-title","aiTitle":{},"sessionId":"{id}"}}"#,
        json!(title)
    )
}

#[cfg(target_os = "linux")]
fn proc_start_of_self() -> String {
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let rest = stat.rsplit_once(')').unwrap().1;
    rest.split_whitespace().nth(19).unwrap().to_string()
}

// ---- data model ------------------------------------------------------------

#[test]
fn title_resolution_order() {
    let env = Env::new();
    env.session(
        A,
        &[
            custom_title(A, "Old name"),
            user("first prompt of A"),
            ai_title(A, "AI name"),
            custom_title(A, "Custom wins"),
            ai_title(A, "Later AI title"),
        ],
    );
    env.session(
        B,
        &[
            user("prompt of B"),
            ai_title(B, "AI one"),
            ai_title(B, "AI two"),
        ],
    );
    let long = "word ".repeat(40);
    env.session(C, &[user(&long)]);
    env.session(D, &[assistant("no prompt here", "m")]);

    let a = env.row(A);
    assert_eq!(
        (a["title"].as_str(), a["title_source"].as_str()),
        (Some("Custom wins"), Some("custom"))
    );
    let b = env.row(B);
    assert_eq!(
        (b["title"].as_str(), b["title_source"].as_str()),
        (Some("AI two"), Some("ai"))
    );
    let c = env.row(C);
    assert_eq!(c["title_source"], "prompt");
    let title = c["title"].as_str().unwrap();
    assert!(title.starts_with("word word"));
    assert!(
        title.chars().count() <= 80,
        "cut to 80 chars including the ellipsis"
    );
    assert!(title.ends_with('…'));
    let d = env.row(D);
    assert_eq!(
        (d["title"].as_str(), d["title_source"].as_str()),
        (Some(""), Some("none"))
    );
}

#[test]
fn title_lookalike_inside_a_message_is_ignored() {
    let env = Env::new();
    let fake = r#"{"type":"x","payload":{"type":"custom-title","customTitle":"fake"}}"#;
    env.session(A, &[user("hello"), fake.into(), ai_title(A, "Real")]);
    let a = env.row(A);
    assert_eq!(a["title"], "Real");
    assert_eq!(a["title_source"], "ai");
}

#[test]
fn prompt_counting_skips_noise_and_tool_results() {
    let env = Env::new();
    env.session(
        A,
        &[
            user("real one"),
            tool_result(),
            user("<command-name>/model</command-name>"),
            user("<local-command-stdout>x</local-command-stdout>"),
            user("<system-reminder>x</system-reminder>"),
            user("Caveat: messages below"),
            user("[Request interrupted by user]"),
            user("   "),
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta"}}"#.into(),
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"summary"}}"#.into(),
            // message before type: not matched by the tool-result skip, still not a prompt
            r#"{"message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"x"}]},"type":"user"}"#.into(),
            // a tool result with a text block next to it is still the harness talking
            r#"{"message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"x"},{"type":"text","text":"Tool loaded."}]},"type":"user"}"#.into(),
            r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"x"},{"type":"text","text":"Tool loaded."}]}}"#.into(),
            user_blocks(&["blocks one", "blocks two"]),
            assistant("an answer", "m"),
            user("last one"),
        ],
    );
    let a = env.row(A);
    assert_eq!(a["prompts"], 3);
    assert_eq!(a["first_prompt"], "real one");
}

#[test]
fn size_counts_transcript_and_sidecar_dir() {
    let env = Env::new();
    env.session(A, &[user("x")]);
    env.write(
        &format!("projects/{SLUG}/{A}/tool-results/big.txt"),
        &"z".repeat(1000),
    );
    let jsonl = fs::metadata(env.path(&format!("projects/{SLUG}/{A}.jsonl")))
        .unwrap()
        .len();
    assert_eq!(env.row(A)["size_bytes"], jsonl + 1000);
}

#[test]
fn list_is_newest_first_and_ignores_memory_dir() {
    let env = Env::new();
    env.session(A, &[user("old")]);
    env.session(B, &[user("new")]);
    env.write(&format!("projects/{SLUG}/memory/MEMORY.md"), "notes");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(env.path(&format!("projects/{SLUG}/{A}.jsonl")))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let ids: Vec<String> = env
        .list()
        .iter()
        .map(|r| r["id"].as_str().unwrap().into())
        .collect();
    assert_eq!(ids, vec![B, A]);
}

#[test]
fn show_reports_prompts_and_metadata() {
    let env = Env::new();
    let mut lines = vec![user("p1"), assistant("a1", "claude-x")];
    lines.extend(["p2", "p3", "p4", "p5"].map(user));
    lines.push(assistant("late", "<synthetic>"));
    env.session(A, &lines);
    let v = env.ok(&["get", A]);
    assert_eq!(v["first_prompts"], json!(["p1", "p2", "p3"]));
    assert_eq!(v["last_prompts"], json!(["p3", "p4", "p5"]));
    assert_eq!(v["cwd"], CWD);
    assert_eq!(v["git_branch"], "feat/x");
    assert_eq!(v["model"], "claude-x");
    assert_eq!(v["prompts"], 5);
    assert!(v["path"].as_str().unwrap().ends_with(&format!("{A}.jsonl")));
}

#[cfg(unix)]
#[test]
fn project_slug_follows_claude_codes_rule() {
    let env = Env::new();
    env.write(
        "projects/-tmp-a-b-c-d/aaaaaaaa-0000-4000-8000-000000000001.jsonl",
        &format!("{}\n", user("hi")),
    );
    assert_eq!(env.items(&["list", "--project", "/tmp/a.b_c d"]).len(), 1);
}

// ---- footprint, dry-run, delete ---------------------------------------------

#[test]
fn delete_removes_exactly_the_dry_run_paths_and_never_memory() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.session(B, &[user("b")]);
    let a_rels = env.full_footprint(A);
    let b_rels = env.full_footprint(B);
    env.write(&format!("projects/{SLUG}/memory/MEMORY.md"), "notes");
    env.write(
        &format!("projects/{SLUG}/memory/{A}.md"),
        "memory named like the id",
    );
    env.write(&format!("todos/{B}-agent.json"), "other");

    let dry = env.ok(&["delete", A, "--dry-run"]);
    assert_eq!(dry["changed"], false);
    assert_eq!(dry["requires_confirmation"], true);
    assert_eq!(dry["targets"][0]["id"], A);
    let listed: Vec<String> = dry["targets"][0]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    assert!(
        listed.iter().all(|p| !slashed(p).contains("/memory")),
        "{listed:?}"
    );
    let before = env.snapshot();
    assert!(
        before.contains_key(&format!("projects/{SLUG}/{A}.jsonl")),
        "dry-run changed nothing"
    );

    assert_eq!(env.kind(&["delete", A]), "confirmation_required");
    assert_eq!(env.snapshot(), before, "no --yes, nothing removed");
    let done = env.ok(&["delete", A, "--yes"]);
    assert_eq!(done["targets"], dry["targets"]);
    assert_eq!(done["changed"], true);
    assert!(done.get("requires_confirmation").is_none());
    let after = env.snapshot();
    let removed: Vec<&String> = before.keys().filter(|k| !after.contains_key(*k)).collect();
    let mut expected: Vec<String> = a_rels.clone();
    expected.push(format!("projects/{SLUG}/{A}.jsonl"));
    // Every removed file is under one of the listed paths, and every listed path is gone.
    for p in &listed {
        assert!(!Path::new(p).exists(), "{p} still exists");
    }
    for rel in &removed {
        let abs = env.path(rel);
        assert!(
            listed.iter().any(|p| abs.starts_with(p)),
            "{rel} removed but not listed"
        );
    }
    for rel in &expected {
        assert!(!after.contains_key(rel), "{rel} should be gone");
    }
    for rel in &b_rels {
        assert!(
            after.contains_key(rel),
            "{rel} of the other session must stay"
        );
    }
    assert!(after.contains_key(&format!("projects/{SLUG}/memory/MEMORY.md")));
    assert!(after.contains_key(&format!("projects/{SLUG}/memory/{A}.md")));
    assert!(after.contains_key(&format!("todos/{B}-agent.json")));
}

#[test]
fn delete_unknown_session_and_bad_id() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    // A missing target fails as such before the confirmation gate.
    assert_eq!(env.kind(&["delete", B]), "not_found");
    assert_eq!(env.kind(&["delete", B, "--yes"]), "not_found");
    assert_eq!(
        env.fails(&["delete", "not-a-uuid"], 2)["kind"],
        "invalid_input"
    );
    assert_eq!(env.fails(&["delete"], 2)["kind"], "invalid_input");
    // A repeat after a delete finds nothing: non_idempotent, declared so.
    env.ok(&["delete", A, "--yes"]);
    assert_eq!(env.kind(&["delete", A, "--yes"]), "not_found");
}

#[test]
fn delete_empty_removes_only_sessions_without_a_conversation() {
    let env = Env::new();
    env.session(A, &[user("<command-name>/clear</command-name>")]);
    env.session(B, &[user("kept")]);
    env.session(C, &[ai_title(C, "titled but empty")]);
    env.session(D, &[]);
    env.session(E, &[user("<command-name>/model</command-name>")]);
    env.full_footprint(C);

    let dry = env.ok(&["delete-empty", "--project", PROJECT, "--dry-run"]);
    assert_eq!(dry["targets"].as_array().unwrap().len(), 4);
    assert_eq!(
        (&dry["changed"], &dry["requires_confirmation"]),
        (&json!(false), &json!(true))
    );
    assert_eq!(env.list().len(), 5, "dry-run deletes nothing");

    assert_eq!(
        env.kind(&["delete-empty", "--project", PROJECT]),
        "confirmation_required"
    );
    assert_eq!(env.list().len(), 5, "no --yes, nothing deleted");
    // --yes is accepted and ignored with --dry-run.
    let still_dry = env.ok(&["delete-empty", "--project", PROJECT, "--dry-run", "--yes"]);
    assert_eq!(still_dry["changed"], false);
    assert_eq!(env.list().len(), 5);

    let done = env.ok(&["delete-empty", "--project", PROJECT, "--yes"]);
    assert_eq!(done["targets"].as_array().unwrap().len(), 4);
    assert_eq!(done["changed"], true);
    // Nothing left to delete: no consent needed, nothing changed.
    let again = env.ok(&["delete-empty", "--project", PROJECT]);
    assert_eq!(
        (&again["targets"], &again["changed"]),
        (&json!([]), &json!(false))
    );
    let ids: Vec<String> = env
        .list()
        .iter()
        .map(|r| r["id"].as_str().unwrap().into())
        .collect();
    assert_eq!(ids, vec![B]);
    assert!(!env.path(&format!("debug/{C}.txt")).exists());
}

#[test]
fn delete_empty_keeps_a_conversation_that_has_no_counted_prompt() {
    let env = Env::new();
    let image = r#"{"type":"user","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]},"cwd":"/work/proj"}"#;
    // Started by a slash command, by `!` input, by a reply alone: 0 prompts each.
    env.session(
        A,
        &[
            user("<command-name>/review</command-name><command-args>PR 12</command-args>"),
            assistant("the review", "m"),
        ],
    );
    env.session(
        B,
        &[
            user("<bash-input>cargo test</bash-input>"),
            assistant("it passes", "m"),
        ],
    );
    env.session(C, &[assistant("only assistant", "m")]);
    // A screenshot nobody answered yet, and an assistant line cut short by a crash.
    env.session(D, &[image.to_string()]);
    env.session(
        E,
        &[r#"{"type":"assistant","message":{"role":"assist"#.to_string()],
    );

    let rows = env.list();
    assert!(rows.iter().all(|r| r["prompts"] == 0), "{rows:?}");
    let dry = env.ok(&["delete-empty", "--project", PROJECT, "--dry-run"]);
    assert_eq!(
        (
            &dry["targets"],
            &dry["skipped"],
            &dry["requires_confirmation"]
        ),
        (&json!([]), &json!([]), &json!(false))
    );
    let done = env.ok(&["delete-empty", "--project", PROJECT, "--yes"]);
    assert_eq!(done["changed"], false);
    assert_eq!(env.list().len(), 5);
}

#[test]
fn a_jsonl_without_a_uuid_name_is_not_a_session() {
    let env = Env::new();
    env.session(B, &[user("kept")]);
    env.write(&format!("projects/{SLUG}/a.jsonl"), "");
    env.write(&format!("projects/{SLUG}/x; touch pwned.jsonl"), "");
    env.write(&format!("security/{A}.json"), "{}");

    assert_eq!(ids_of(&env.list()), vec![B]);
    let done = env.ok(&["delete-empty", "--project", PROJECT]);
    assert_eq!(done["targets"], json!([]));
    assert!(env.path(&format!("projects/{SLUG}/a.jsonl")).exists());
    assert!(
        env.path(&format!("security/{A}.json")).exists(),
        "glob *a* must not match"
    );
}

// ---- guards ----------------------------------------------------------------

#[test]
fn current_session_is_refused_by_every_destructive_command() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    let before = env.snapshot();
    for args in [
        vec!["archive", A, "--current", A],
        vec!["delete", A, "--current", A],
        vec!["delete", A, "--yes", "--current", A],
        vec!["rename", A, "new name", "--current", A],
    ] {
        let err = env.fails(&args, 1);
        assert_eq!(err["kind"], "session_live", "{args:?}");
        assert!(err["message"].as_str().unwrap().contains("current"));
        assert_eq!(err["context"]["id"], A);
    }
    // Restoring a session that is not archived changes nothing, so nothing to refuse.
    assert_eq!(env.ok(&["restore", A, "--current", A])["changed"], false);
    assert_eq!(env.snapshot(), before);
}

// The start-time check needs /proc; elsewhere only the pid is checked.
#[cfg(target_os = "linux")]
#[test]
fn live_session_is_refused_and_flagged_in_list() {
    let env = Env::new();
    env.session(A, &[user("live one")]);
    env.session(B, &[user("stale pid")]);
    env.session(C, &[user("pid reused")]);
    env.live(A, std::process::id(), Some(&proc_start_of_self()));
    env.live(B, 999_999_999, Some("1"));
    // Same live pid as A but a different start time: the pid was reused by another process.
    env.write(
        "sessions/pid-reused.json",
        &json!({"pid": std::process::id(), "sessionId": C, "procStart": "12345"}).to_string(),
    );

    assert_eq!(env.row(A)["is_live"], true);
    assert_eq!(env.row(B)["is_live"], false, "dead pid");
    assert_eq!(
        env.row(C)["is_live"],
        false,
        "start time differs, pid was reused"
    );

    let before = env.snapshot();
    for args in [
        vec!["archive", A],
        vec!["delete", A, "--yes"],
        vec!["rename", A, "x"],
    ] {
        let err = env.fails(&args, 1);
        assert_eq!(err["kind"], "session_live", "{args:?}");
        assert!(err["message"].as_str().unwrap().contains("live"));
    }
    assert_eq!(env.snapshot(), before);

    env.ok(&["rename", B, "allowed"]);
    env.ok(&["delete", C, "--yes"]);
}

#[test]
fn delete_empty_skips_current_and_live() {
    let env = Env::new();
    env.session(A, &[]);
    env.session(B, &[]);
    env.session(C, &[]);
    env.live(A, std::process::id(), None);
    let done = env.ok(&[
        "delete-empty",
        "--project",
        PROJECT,
        "--current",
        B,
        "--yes",
    ]);
    assert_eq!(done["targets"].as_array().unwrap().len(), 1);
    assert_eq!(done["targets"][0]["id"], C);
    let skipped: Vec<&str> = done["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(skipped.len(), 2);
    assert!(skipped.contains(&A) && skipped.contains(&B));
    assert_eq!(env.list().len(), 2);
}

// ---- pins ------------------------------------------------------------------

fn ids_of(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap().into())
        .collect()
}

#[test]
fn a_pin_keeps_a_session_on_top_through_limit_archive_and_restore() {
    let env = Env::new();
    env.session(A, &[user("oldest one")]);
    env.session(B, &[user("newer")]);
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(env.path(&format!("projects/{SLUG}/{A}.jsonl")))
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(ids_of(&env.list()), vec![B, A]);

    assert_eq!(env.ok(&["pin", A])["changed"], true);
    assert_eq!(
        env.ok(&["pin", A])["changed"],
        false,
        "pinning twice is a no-op"
    );
    assert_eq!(ids_of(&env.list()), vec![A, B]);
    assert_eq!(env.row(A)["is_pinned"], true);
    assert_eq!(env.row(B)["is_pinned"], false);
    let limited = env.ok(&["list", "--project", PROJECT, "--limit", "1"]);
    assert_eq!(limited["items"][0]["id"], A, "the limit keeps pins");
    assert_eq!(limited["has_more"], true);
    assert_eq!(env.ok(&["get", A])["is_pinned"], true);
    assert_eq!(
        env.items(&["search", "oldest", "--project", PROJECT])[0]["is_pinned"],
        true
    );

    env.ok(&["archive", A]);
    let archived = env.items(&["list", "--archived", "--project", PROJECT]);
    assert_eq!(
        archived[0]["is_pinned"], true,
        "the pin follows the id into the archive"
    );
    env.ok(&["restore", A]);
    assert_eq!(env.row(A)["is_pinned"], true);

    assert_eq!(env.ok(&["unpin", A])["changed"], true);
    assert_eq!(ids_of(&env.list()), vec![B, A]);
    assert_eq!(env.ok(&["unpin", A])["changed"], false);
}

#[test]
fn pin_needs_an_existing_session_and_delete_drops_the_pin() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    assert_eq!(env.kind(&["pin", B]), "not_found");
    assert_eq!(env.fails(&["pin", "nope"], 2)["kind"], "invalid_input");

    env.ok(&["pin", A]);
    env.ok(&["delete", A, "--dry-run"]);
    assert!(
        fs::read_to_string(env.path("sessile/pins.json"))
            .unwrap()
            .contains(A)
    );
    env.ok(&["delete", A, "--yes"]);
    assert!(
        !fs::read_to_string(env.path("sessile/pins.json"))
            .unwrap()
            .contains(A)
    );
}

#[test]
fn delete_empty_skips_pinned() {
    let env = Env::new();
    env.session(A, &[]);
    env.session(B, &[]);
    env.ok(&["pin", A]);
    let done = env.ok(&["delete-empty", "--project", PROJECT, "--yes"]);
    assert_eq!(done["targets"].as_array().unwrap().len(), 1);
    assert_eq!(done["targets"][0]["id"], B);
    assert_eq!(done["skipped"][0]["id"], A);
    assert_eq!(done["skipped"][0]["reason"], "pinned");
    assert_eq!(ids_of(&env.list()), vec![A]);
}

#[test]
fn a_broken_pins_file_is_reported_and_delete_changes_nothing() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.write("sessile/pins.json", "{not json");
    // Reading still works: the rows arrive, none marked pinned, then the call fails.
    for args in [
        vec!["list", "--project", PROJECT],
        vec!["search", "a", "--project", PROJECT],
        vec!["get", A],
    ] {
        let mut full = args.clone();
        full.push("--json");
        let out = env.run(&full);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let row = if args[0] == "get" {
            &v
        } else {
            assert_eq!(v["partial"], true, "{args:?}");
            &v["items"][0]
        };
        assert_eq!(row["id"], A, "{args:?}");
        assert_eq!(row["is_pinned"], false, "{args:?}");
        let err = &last_line_json(&out.stderr)["error"];
        assert_eq!(err["kind"], "pins_unreadable", "{args:?}");
    }
    assert_eq!(env.kind(&["pin", A]), "pins_unreadable");
    let err = env.fails(&["unpin", A], 1);
    assert_eq!(err["kind"], "pins_unreadable");
    assert!(err["hint"].as_str().unwrap().contains("remove it"));
    assert!(
        err["context"]["path"]
            .as_str()
            .unwrap()
            .ends_with("pins.json")
    );
    let before = env.snapshot();
    assert_eq!(env.kind(&["delete", A, "--yes"]), "pins_unreadable");
    // The pins check runs before the confirmation gate.
    assert_eq!(env.kind(&["delete", A]), "pins_unreadable");
    assert_eq!(env.snapshot(), before);
}

// ---- archive and restore ------------------------------------------------------

#[test]
fn archive_then_restore_round_trips_the_footprint() {
    let env = Env::new();
    env.session(A, &[user("a"), ai_title(A, "Archived one")]);
    env.session(B, &[user("b")]);
    env.full_footprint(A);
    env.write(&format!("projects/{SLUG}/memory/MEMORY.md"), "notes");
    let before = env.snapshot();

    let moved = env.ok(&["archive", A]);
    assert_eq!(moved["changed"], true);
    let mid = env.snapshot();
    let again = env.ok(&["archive", A]);
    assert_eq!(
        (&again["changed"], &again["moved"]),
        (&json!(false), &json!([])),
        "archive is idempotent"
    );
    assert_eq!(env.snapshot(), mid);
    assert!(!mid.contains_key(&format!("projects/{SLUG}/{A}.jsonl")));
    assert!(mid.contains_key(&format!("session-archive/{SLUG}/projects/{SLUG}/{A}.jsonl")));
    assert!(mid.contains_key(&format!("session-archive/{SLUG}/session-env/{A}/env")));
    assert!(
        mid.contains_key(&format!("projects/{SLUG}/{B}.jsonl")),
        "other session stays"
    );
    assert!(
        mid.contains_key(&format!("projects/{SLUG}/memory/MEMORY.md")),
        "memory stays"
    );
    assert!(env.list().iter().all(|r| r["id"] != A));

    let archived = env.items(&["list", "--project", PROJECT, "--archived"]);
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0]["id"], A);
    assert_eq!(archived[0]["title"], "Archived one");
    assert_eq!(env.ok(&["get", A, "--archived"])["id"], A);

    assert_eq!(env.ok(&["restore", A])["changed"], true);
    assert_eq!(env.snapshot(), before, "restore puts every file back");
    assert_eq!(
        env.ok(&["restore", A])["changed"],
        false,
        "restore is idempotent"
    );
    assert_eq!(env.snapshot(), before);
    assert_eq!(env.kind(&["restore", C]), "not_found");
    assert_eq!(env.kind(&["archive", C]), "not_found");
    assert!(
        !env.path(&format!("session-archive/{SLUG}")).exists(),
        "emptied archive dirs are pruned"
    );
}

#[test]
fn restore_refuses_on_collision_and_moves_nothing() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.full_footprint(A);
    env.ok(&["archive", A]);
    // A new file appears where one of the archived paths belongs.
    env.write(&format!("debug/{A}.txt"), "new");
    let before = env.snapshot();

    let err = env.fails(&["restore", A], 1);
    assert_eq!(err["kind"], "conflict");
    let paths = err["context"]["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 1);
    assert!(slashed(paths[0].as_str().unwrap()).ends_with(&format!("debug/{A}.txt")));
    assert_eq!(env.snapshot(), before);
}

#[test]
fn archive_refuses_when_destination_exists() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.write(
        &format!("session-archive/{SLUG}/projects/{SLUG}/{A}.jsonl"),
        "older",
    );
    let before = env.snapshot();
    let err = env.fails(&["archive", A], 1);
    assert_eq!(err["kind"], "conflict");
    assert!(
        err["hint"]
            .as_str()
            .unwrap()
            .contains("after it was archived")
    );
    assert_eq!(env.snapshot(), before);
}

#[test]
fn hints_never_name_a_flag_the_command_lacks() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.ok(&["archive", A]);
    let err = env.fails(&["rename", A, "New"], 1);
    assert_eq!(err["kind"], "not_found");
    assert!(err["message"].as_str().unwrap().contains("is archived"));
    assert!(err["hint"].as_str().unwrap().contains("sessile restore"));
    for args in [vec!["rename", B, "New"], vec!["pin", B]] {
        let err = env.fails(&args, 1);
        assert_eq!(err["kind"], "not_found", "{args:?}");
        assert!(!err["hint"].as_str().unwrap().contains("--archived "));
        assert!(err["message"].as_str().unwrap().contains("neither"));
    }
}

#[test]
fn pins_made_at_the_same_time_are_all_kept() {
    let env = Env::new();
    let ids: Vec<String> = (0..16)
        .map(|n| format!("{n:08x}-1111-4222-8333-444455556666"))
        .collect();
    for id in &ids {
        env.session(id, &[user("x")]);
    }
    std::thread::scope(|scope| {
        for id in &ids {
            let env = &env;
            scope.spawn(move || env.ok(&["pin", id]));
        }
    });
    let stored = fs::read_to_string(env.path("sessile/pins.json")).unwrap();
    for id in &ids {
        assert!(stored.contains(id.as_str()), "the pin of {id} was lost");
    }
}

#[test]
fn an_id_in_upper_case_is_the_same_session() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    let upper = A.to_uppercase();
    assert_eq!(env.ok(&["get", &upper])["id"], A);
    assert_eq!(
        env.kind(&["archive", A, "--current", &upper]),
        "session_live"
    );
    env.live(A, std::process::id(), None);
    assert_eq!(env.kind(&["delete", &upper, "--yes"]), "session_live");
}

#[test]
fn a_session_file_that_names_no_session_counts_as_live() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.session(B, &[]);
    // As caught while Claude Code rewrites it: this process is alive, its file is torn.
    env.write(
        &format!("sessions/{}.json", std::process::id()),
        "{\"pid\":",
    );
    let before = env.snapshot();
    for args in [
        vec!["archive", A],
        vec!["rename", A, "New"],
        vec!["delete", A, "--yes"],
    ] {
        let err = env.fails(&args, 1);
        assert_eq!(err["kind"], "session_live", "{args:?}");
        assert!(err["message"].as_str().unwrap().contains("does not name"));
    }
    let done = env.ok(&["delete-empty", "--project", PROJECT, "--yes"]);
    assert_eq!(done["changed"], false);
    assert_eq!(done["skipped"][0]["id"], B);
    assert_eq!(env.snapshot(), before);
    // The same torn file of a process that is gone holds nothing back.
    fs::remove_file(env.path(&format!("sessions/{}.json", std::process::id()))).unwrap();
    env.write("sessions/999999999.json", "{\"pid\":");
    env.ok(&["archive", A]);
}

#[test]
fn delete_takes_the_transcript_last() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.full_footprint(A);
    env.ok(&["export", A]);
    let dry = env.ok(&["delete", A, "--dry-run"]);
    let paths = dry["targets"][0]["paths"].as_array().unwrap();
    let last = slashed(paths.last().unwrap().as_str().unwrap());
    assert!(
        last.ends_with(&format!("projects/{SLUG}/{A}.jsonl")),
        "{last}"
    );
}

#[test]
fn delete_archived_session() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.full_footprint(A);
    env.ok(&["archive", A]);
    assert_eq!(env.kind(&["delete", A, "--yes"]), "not_found");
    env.ok(&["delete", A, "--archived", "--yes"]);
    assert!(!env.path(&format!("session-archive/{SLUG}")).exists());
}

// ---- rename ----------------------------------------------------------------

#[test]
fn rename_appends_a_custom_title_entry() {
    let env = Env::new();
    env.session(A, &[user("a"), ai_title(A, "AI name")]);
    // A transcript whose last line is torn must still get a clean entry on its own line.
    let path = env.path(&format!("projects/{SLUG}/{A}.jsonl"));
    let mut body = fs::read_to_string(&path).unwrap();
    body.pop();
    fs::write(&path, body).unwrap();

    let done = env.ok(&["rename", A, "My", "new \"name\""]);
    assert_eq!(
        (&done["changed"], &done["title"]),
        (&json!(true), &json!("My new \"name\""))
    );
    let content = fs::read_to_string(&path).unwrap();
    // The same title again changes nothing and appends nothing.
    assert_eq!(env.ok(&["rename", A, "My new \"name\""])["changed"], false);
    assert_eq!(fs::read_to_string(&path).unwrap(), content);
    // A title is one line of bounded length; a refused one appends nothing.
    let long = "x".repeat(201);
    for bad in ["two\nlines", "bell\u{7}", long.as_str()] {
        assert_eq!(
            env.fails(&["rename", A, bad], 2)["kind"],
            "invalid_input",
            "{bad:?}"
        );
    }
    env.ok(&["rename", A, &"x".repeat(200)]);
    env.ok(&["rename", A, "My new \"name\""]);
    let content = fs::read_to_string(&path).unwrap();
    let last = content.lines().last().unwrap();
    let v: Value = serde_json::from_str(last).unwrap();
    assert_eq!(
        v,
        json!({"type": "custom-title", "customTitle": "My new \"name\"", "sessionId": A})
    );
    assert!(
        last.starts_with(r#"{"type":"custom-title","customTitle""#),
        "same key order as /rename"
    );
    assert!(content.ends_with('\n'));
    let row = env.row(A);
    assert_eq!(
        (row["title"].as_str(), row["title_source"].as_str()),
        (Some("My new \"name\""), Some("custom"))
    );
    assert_eq!(row["prompts"], 1);
}

// ---- search ----------------------------------------------------------------

#[test]
fn search_ranks_title_over_prompt_over_assistant() {
    let env = Env::new();
    env.session(
        A,
        &[
            user("unrelated"),
            assistant("we discussed the quokka habitat at length", "m"),
        ],
    );
    env.session(B, &[user("tell me about quokka diets please")]);
    env.session(C, &[user("hello"), custom_title(C, "Quokka research")]);
    env.session(D, &[user("nothing relevant")]);

    let hits = env.items(&["search", "quokka", "--project", PROJECT]);
    let ids: Vec<&str> = hits.iter().map(|h| h["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![C, B, A]);
    assert_eq!(hits[0]["matched_in"], "title");
    assert_eq!(hits[1]["matched_in"], "prompt");
    assert_eq!(hits[2]["matched_in"], "assistant");
    assert!(
        hits[2]["snippet"]
            .as_str()
            .unwrap()
            .contains("quokka habitat")
    );
    assert!(hits[0]["score"].as_u64() > hits[1]["score"].as_u64());
    assert!(hits[1]["score"].as_u64() > hits[2]["score"].as_u64());
    assert_eq!(hits[0]["prompts"], 1, "hits carry the list row fields");
}

#[test]
fn search_is_fuzzy_on_titles_and_needs_all_words() {
    let env = Env::new();
    env.session(
        A,
        &[user("x"), custom_title(A, "Refactor the session parser")],
    );
    env.session(B, &[user("x"), assistant("the parser is fine", "m")]);
    let fuzzy = env.items(&["search", "refactr parsr", "--project", PROJECT]);
    assert_eq!(fuzzy[0]["id"], A);
    let both = env.items(&["search", "parser session", "--project", PROJECT]);
    assert_eq!(ids_of(&both), vec![A], "B has only one of the words");
    let none = env.ok(&["search", "zzzzqqq", "--project", PROJECT]);
    assert_eq!(
        none,
        json!({"items": [], "has_more": false, "partial": false}),
        "an empty page keeps its shape"
    );
}

fn sorted_ids(rows: &[Value]) -> Vec<String> {
    let mut ids = ids_of(rows);
    ids.sort();
    ids
}

fn headless_user(text: &str, entrypoint: &str) -> String {
    format!(
        r#"{{"type":"user","message":{{"role":"user","content":{}}},"cwd":"/work/proj","entrypoint":"{entrypoint}"}}"#,
        json!(text)
    )
}

fn user_at(text: &str, timestamp: &str) -> String {
    format!(
        r#"{{"type":"user","message":{{"role":"user","content":{}}},"cwd":"/work/proj","timestamp":"{timestamp}"}}"#,
        json!(text)
    )
}

#[test]
fn rows_carry_the_start_time_of_the_session() {
    let env = Env::new();
    env.session(
        A,
        &[
            r#"{"type":"permission-mode","permissionMode":"default"}"#.to_string(),
            user_at("deploy check", "2026-09-19T18:50:40.078Z"),
            user_at("deploy again", "2026-10-01T08:00:00.000Z"),
        ],
    );
    env.session(B, &[user("deploy check")]);
    env.session(C, &[user_at("deploy check", "yesterday")]);
    let rows = env.list();
    let created = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap()["created_at"].clone();
    assert_eq!(
        created(A),
        "2026-09-19T18:50:40.078Z",
        "the first entry, not the last"
    );
    assert_eq!(created(B), Value::Null);
    assert_eq!(
        created(C),
        Value::Null,
        "an unknown format is not passed on"
    );
    assert_eq!(
        env.ok(&["get", A])["created_at"],
        "2026-09-19T18:50:40.078Z"
    );
    let hits = env.items(&["search", "deploy", "--project", PROJECT]);
    assert!(
        hits.iter()
            .any(|h| h["created_at"] == "2026-09-19T18:50:40.078Z")
    );
}

#[test]
fn headless_sessions_are_hidden_unless_asked_for_or_pinned() {
    let env = Env::new();
    env.session(A, &[headless_user("deploy check", "cli")]);
    env.session(B, &[headless_user("deploy check", "sdk-cli")]);
    env.session(C, &[headless_user("deploy check", "sdk-ts")]);
    env.session(D, &[user("deploy check")]);
    let shown = env.list();
    assert_eq!(
        sorted_ids(&shown),
        vec![A, D],
        "an unknown entrypoint is not hidden"
    );
    let interactive =
        |id: &str| shown.iter().find(|r| r["id"] == id).unwrap()["interactive"].clone();
    assert_eq!(interactive(A), true);
    assert_eq!(
        interactive(D),
        Value::Null,
        "older Claude Code wrote no entrypoint"
    );
    let all = env.items(&["list", "--project", PROJECT, "--include-headless"]);
    assert_eq!(sorted_ids(&all), vec![A, B, C, D]);
    let b = all.iter().find(|r| r["id"] == B).unwrap();
    assert_eq!(b["interactive"], false);
    let hits = env.items(&["search", "deploy", "--project", PROJECT]);
    assert_eq!(sorted_ids(&hits), vec![A, D]);
    env.ok(&["pin", C]);
    assert_eq!(
        sorted_ids(&env.list()),
        vec![A, C, D],
        "a pin keeps it in view"
    );
    assert_eq!(env.ok(&["get", B])["interactive"], false);
}

#[test]
fn search_in_limits_the_fields_and_prompts_are_not_fuzzy() {
    let env = Env::new();
    env.session(A, &[user("x"), custom_title(A, "grpc dispatch notes")]);
    env.session(B, &[user("run grpc with curl")]);
    env.session(C, &[user("x"), assistant("then grpc reported done", "m")]);
    env.session(
        D,
        &[user(
            "I want to cherry-pick commit abc123 from develop into the release branch",
        )],
    );
    let every = env.items(&["search", "grpc", "--project", PROJECT]);
    assert_eq!(
        ids_of(&every),
        vec![A, B, C],
        "title x3, prompt x2, reply x1"
    );
    let fields: Vec<&str> = every
        .iter()
        .map(|h| h["matched_in"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["title", "prompt", "assistant"]);
    let asked = env.items(&[
        "search",
        "grpc",
        "--project",
        PROJECT,
        "--in",
        "title,prompt",
    ]);
    assert_eq!(ids_of(&asked), vec![A, B]);
    let repeated = env.items(&[
        "search",
        "grpc",
        "--project",
        PROJECT,
        "--in",
        "title",
        "--in",
        "assistant",
    ]);
    assert_eq!(ids_of(&repeated), vec![A, C]);
    assert_eq!(
        env.fails(&["search", "grpc", "--in", "body"], 2)["kind"],
        "invalid_input"
    );
}

#[test]
fn an_exact_hit_always_outranks_a_typo_in_a_title() {
    let env = Env::new();
    // A real false positive of subsequence matching: "grpc" as scattered letters.
    env.session(A, &[user("x"), custom_title(A, "Group report on pricing")]);
    env.session(B, &[user("x"), assistant("grpc", "m")]);
    let short = env.items(&["search", "grpc", "--project", PROJECT]);
    assert_eq!(ids_of(&short), vec![B]);
    assert_eq!(short[0]["fuzzy"], false);
    env.session(
        C,
        &[user("x"), custom_title(C, "Refactor the session parser")],
    );
    let typo = env.items(&["search", "refactr parsr", "--project", PROJECT]);
    assert_eq!(ids_of(&typo), vec![C]);
    assert_eq!(typo[0]["fuzzy"], true);
    env.session(D, &[user("x"), assistant("then refactr parsr ran", "m")]);
    let mixed = env.items(&["search", "refactr parsr", "--project", PROJECT]);
    assert_eq!(
        ids_of(&mixed),
        vec![D, C],
        "a reply with the words as written beats a typo in a title"
    );
    assert!(mixed[0]["score"].as_u64() > mixed[1]["score"].as_u64());
}

#[test]
fn typos_match_title_words_and_rank_by_distance() {
    let env = Env::new();
    env.session(A, &[user("x"), custom_title(A, "Sessile CLI conformance")]);
    env.session(B, &[user("x"), custom_title(B, "sesile notes")]);
    env.session(
        C,
        &[user("x"), custom_title(C, "release-create skill review")],
    );
    // The AI title behind D's custom one is not what the row shows.
    env.session(
        D,
        &[
            user("x"),
            ai_title(D, "sesille work"),
            custom_title(D, "notes"),
        ],
    );
    let hits = env.items(&["search", "sesille", "--project", PROJECT]);
    assert_eq!(ids_of(&hits), vec![B, A], "one edit ranks above two");
    assert!(hits.iter().all(|h| h["fuzzy"] == true));
    assert!(hits[0]["score"].as_u64() > hits[1]["score"].as_u64());
    env.session(E, &[user("x"), custom_title(E, "changelog dev")]);
    env.session(
        C,
        &[
            user("x"),
            custom_title(C, "Changelog skill tests on staging"),
        ],
    );
    let swapped = env.items(&["search", "changelgo", "--project", PROJECT]);
    assert_eq!(
        sorted_ids(&swapped),
        vec![C, E],
        "every title with the word"
    );
    let far = env.items(&["search", "skilx", "--project", PROJECT]);
    assert_eq!(ids_of(&far), vec![C], "5-6 letters allow one edit");
    assert!(
        env.items(&["search", "skixx", "--project", PROJECT])
            .is_empty()
    );
}

#[test]
fn typos_match_only_when_no_title_has_the_words_as_written() {
    let env = Env::new();
    env.session(A, &[user("x"), custom_title(A, "sessile dev")]);
    env.session(B, &[user("x"), custom_title(B, "Generate session title")]);
    env.session(C, &[user("why does sesille fail"), custom_title(C, "grpc")]);
    let exact = env.items(&["search", "sessile", "--project", PROJECT]);
    assert_eq!(ids_of(&exact), vec![A], "session is two edits from sessile");
    let typo = env.items(&["search", "sesille", "--project", PROJECT]);
    assert_eq!(
        ids_of(&typo),
        vec![C, A],
        "an exact prompt hit keeps typo titles"
    );
    assert_eq!(typo[1]["fuzzy"], true);
}

#[test]
fn min_prompts_drops_one_shot_sessions() {
    let env = Env::new();
    env.session(A, &[user("deploy one")]);
    env.session(B, &[user("deploy one"), user("two"), user("three")]);
    env.session(C, &[user("deploy")]);
    env.ok(&["pin", C]);
    let rows = env.items(&["list", "--project", PROJECT, "--min-prompts", "3"]);
    assert_eq!(
        ids_of(&rows),
        vec![B],
        "a pin does not exempt from an explicit filter"
    );
    let hits = env.items(&[
        "search",
        "deploy",
        "--project",
        PROJECT,
        "--min-prompts",
        "2",
    ]);
    assert_eq!(ids_of(&hits), vec![B]);
}

#[test]
fn the_current_session_comes_from_claude_code_unless_given() {
    let env = Env::new();
    env.session(A, &[user("deploy")]);
    env.session(B, &[user("deploy")]);
    let run = |args: &[&str], session: &str| -> Output {
        Command::new(env!("CARGO_BIN_EXE_sessile"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", env.root())
            .env("CLAUDE_CODE_SESSION_ID", session)
            .output()
            .unwrap()
    };
    let current = |extra: &[&str], session: &str| -> Vec<String> {
        let mut args = vec!["list", "--project", PROJECT, "--json"];
        args.extend(extra);
        let v: Value = serde_json::from_slice(&run(&args, session).stdout).unwrap();
        let marked: Vec<Value> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["is_current"] == true)
            .cloned()
            .collect();
        assert!(marked.iter().all(|r| r["is_live"] == true));
        sorted_ids(&marked)
    };
    assert_eq!(current(&[], A), vec![A]);
    assert_eq!(current(&["--current", B], A), vec![B], "the flag wins");
    assert!(current(&[], "").is_empty());
    let out = run(&["archive", A, "--json"], A);
    assert_eq!(last_line_json(&out.stderr)["error"]["kind"], "session_live");
}

#[test]
fn the_schema_says_which_flags_exclude_each_other() {
    let env = Env::new();
    let detail = env.schema(&["export"]);
    let described = |name: &str| {
        let flags = detail["flags"].as_array().unwrap();
        let flag = flags.iter().find(|f| f["name"] == name).unwrap();
        flag["description"].as_str().unwrap().to_string()
    };
    assert!(described("turns").contains("Conflicts with --last-turns"));
    assert!(described("last-turns").contains("Conflicts with --turns"));
}

#[test]
fn a_timestamp_that_is_no_time_and_an_sdk_lookalike_are_not_taken() {
    let env = Env::new();
    env.session(A, &[user_at("x", "2026-99-99T99:99:99.999Z")]);
    env.session(B, &[headless_user("y", "sdkish")]);
    assert_eq!(env.row(A)["created_at"], Value::Null);
    assert_eq!(env.row(B)["interactive"], true);
}

#[cfg(target_os = "linux")]
#[test]
fn a_result_that_cannot_be_written_fails_with_the_error_object() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    let out = Command::new(env!("CARGO_BIN_EXE_sessile"))
        .args(["list", "--project", PROJECT])
        .env("CLAUDE_CONFIG_DIR", env.root())
        .stdout(
            fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = &last_line_json(&out.stderr)["error"];
    assert_eq!(err["kind"], "io_error");
    assert!(err["message"].as_str().unwrap().contains("cannot write"));
}

#[test]
fn search_in_is_described_as_a_repeatable_choice() {
    let env = Env::new();
    let detail = env.schema(&["search"]);
    let flag = detail["flags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "in")
        .unwrap()
        .clone();
    assert_eq!(flag["enum"], json!(["title", "prompt", "assistant"]));
    assert_eq!(flag["default"], json!(["title", "prompt", "assistant"]));
    assert_eq!(flag["repeatable"], true);
    assert_eq!(flag["type"], "string");
}

#[test]
fn search_limit_and_empty_query() {
    let env = Env::new();
    for (i, id) in [A, B, C].iter().enumerate() {
        env.session(id, &[user(&format!("shared word {i}"))]);
    }
    let v = env.ok(&["search", "shared", "--project", PROJECT, "--limit", "2"]);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["has_more"], true);
    let all = env.ok(&["search", "shared", "--project", PROJECT, "--limit", "3"]);
    assert_eq!(all["has_more"], false);
    assert_eq!(
        env.fails(&["search", "  ", "--project", PROJECT], 2)["kind"],
        "invalid_input"
    );
}

#[test]
fn global_flags_work_on_every_subcommand_and_project_defaults_to_cwd() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.session(B, &[]);
    // The mod passes --project, --current and --json to everything.
    let flags = ["--project", PROJECT, "--current", C, "--json"];
    for args in [
        vec!["list"],
        vec!["get", A],
        vec!["search", "a"],
        vec!["rename", A, "x"],
        vec!["delete", A, "--dry-run"],
        vec!["delete-empty", "--dry-run"],
        vec!["archive", A],
        vec!["restore", A],
        vec!["list", "--all"],
        vec!["search", "a", "--all"],
        vec!["export", A],
    ] {
        let mut full = args.clone();
        full.extend(flags);
        let out = env.run(&full);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    // No --project: the cwd decides the project.
    let out = Command::new(env!("CARGO_BIN_EXE_sessile"))
        .args(["list", "--json"])
        .current_dir(env.root())
        .env("CLAUDE_CONFIG_DIR", env.root())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"items": [], "has_more": false, "partial": false}),
        "the temp dir is not a project"
    );
}

#[test]
fn project_scopes_the_id_lookup_when_given() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.write(
        &format!("projects/{OTHER_SLUG}/{B}.jsonl"),
        &format!("{}\n", user("b")),
    );
    // A belongs to PROJECT: found there and without --project, not found in OTHER.
    assert_eq!(env.ok(&["get", A, "--project", PROJECT])["id"], A);
    assert_eq!(env.ok(&["get", A])["id"], A);
    for args in [
        vec!["get", A],
        vec!["rename", A, "x"],
        vec!["archive", A],
        vec!["delete", A, "--dry-run"],
        vec!["export", A],
    ] {
        let mut full = args.clone();
        full.extend(["--project", OTHER]);
        assert_eq!(env.kind(&full), "not_found", "{args:?}");
    }
    assert!(env.path(&format!("projects/{SLUG}/{A}.jsonl")).exists());
    // B is only reachable through its own project, with or without the flag.
    let paths_of = |v: Value| v["targets"][0]["paths"].as_array().unwrap().len();
    assert_eq!(
        paths_of(env.ok(&["delete", B, "--dry-run", "--project", OTHER])),
        1
    );
    assert_eq!(paths_of(env.ok(&["delete", B, "--dry-run"])), 1);
    // Restore scopes to the archive of that project.
    env.ok(&["archive", A, "--project", PROJECT]);
    assert_eq!(env.kind(&["restore", A, "--project", OTHER]), "not_found");
    env.ok(&["restore", A, "--project", PROJECT]);
}

#[test]
fn delete_contract_and_empty_rows_are_list_rows() {
    let env = Env::new();
    env.session(A, &[]);
    env.write(&format!("debug/{A}.txt"), "x");
    let dry = env.ok(&["delete", A, "--dry-run"]);
    let real = env.ok(&["delete", A, "--yes"]);
    assert_eq!(dry["targets"], real["targets"]);
    assert_eq!(real["targets"][0]["paths"].as_array().unwrap().len(), 2);

    env.session(B, &[]);
    let dry = env.ok(&["delete-empty", "--project", PROJECT, "--dry-run"]);
    let row = &dry["targets"][0];
    assert_eq!(row["id"], B);
    for key in [
        "title",
        "title_source",
        "updated_at",
        "size_bytes",
        "prompts",
        "is_live",
        "paths",
    ] {
        assert!(row.get(key).is_some(), "missing {key}");
    }
}

#[test]
fn current_session_is_reported_live_before_it_has_a_pid_file() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    env.session(B, &[user("b")]);
    let v = env.items(&["list", "--project", PROJECT, "--current", A]);
    let live: Vec<(&str, bool)> = v
        .iter()
        .map(|r| (r["id"].as_str().unwrap(), r["is_live"].as_bool().unwrap()))
        .collect();
    assert!(
        live.contains(&(A, true)) && live.contains(&(B, false)),
        "{live:?}"
    );
    assert_eq!(env.ok(&["get", A, "--current", A])["is_live"], true);
    let hits = env.items(&["search", "a", "--project", PROJECT, "--current", A]);
    assert!(hits.iter().any(|h| h["id"] == A && h["is_live"] == true));
}

// ---- every project, export ---------------------------------------------------

fn tool_use(name: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","model":"m","content":[{{"type":"tool_use","id":"t","name":"{name}","input":{{}}}}]}}}}"#
    )
}

#[test]
fn all_lists_and_searches_every_project_and_rows_carry_their_dir() {
    let env = Env::new();
    env.session(A, &[user("alpha here")]);
    env.write(
        &format!("projects/{OTHER_SLUG}/{B}.jsonl"),
        &format!("{}\n", user("alpha there").replace(CWD, "/other/proj")),
    );
    assert_eq!(env.list().len(), 1);
    let rows = env.items(&["list", "--all", "--project", PROJECT]);
    assert_eq!(rows.len(), 2);
    assert_eq!(env.items(&["list", "--all", "--limit", "1"]).len(), 1);
    let b = rows.iter().find(|r| r["id"] == B).unwrap();
    assert_eq!(b["cwd"], "/other/proj");
    let resume = b["resume_command"].as_str().unwrap();
    assert!(
        resume.starts_with("cd '/other/proj'") && resume.ends_with(&format!("claude --resume {B}"))
    );

    assert_eq!(
        env.items(&["search", "alpha", "--all", "--project", PROJECT])
            .len(),
        2
    );
    assert_eq!(
        env.items(&["search", "alpha", "--project", PROJECT]).len(),
        1
    );

    env.ok(&["archive", B]);
    let archived = env.items(&["list", "--all", "--archived"]);
    assert_eq!(archived[0]["id"], B);
}

fn three_turns(env: &Env) {
    env.session(
        A,
        &[
            user("set up the router"),
            assistant("The router config is in place.", "claude-x"),
            user("<system-reminder>router injected</system-reminder>"),
            user("now the docs"),
            assistant("Docs written.", "claude-x"),
            user("did the Router survive the reboot?"),
            assistant("Yes.", "claude-x"),
        ],
    );
}

#[test]
fn excerpts_give_the_turns_where_words_come_up() {
    let env = Env::new();
    three_turns(&env);
    let found = env.items(&["excerpts", A, "router"]);
    let at: Vec<(u64, &str)> = found
        .iter()
        .map(|e| (e["turn"].as_u64().unwrap(), e["role"].as_str().unwrap()))
        .collect();
    assert_eq!(
        at,
        vec![(1, "prompt"), (1, "assistant"), (3, "prompt")],
        "injected text is no turn"
    );
    assert!(
        found[2]["snippet"]
            .as_str()
            .unwrap()
            .contains("Router survive")
    );
    let page = env.ok(&["excerpts", A, "router", "--limit", "1"]);
    assert_eq!(page["has_more"], true);
    assert_eq!(page["partial"], false);
    assert!(
        env.items(&["excerpts", A, "router docs"]).is_empty(),
        "all words in one message"
    );
    let plain = env.run(&["excerpts", A, "docs", "--plain"]);
    assert_eq!(
        String::from_utf8(plain.stdout).unwrap(),
        "2\tprompt\tnow the docs\n2\tassistant\tDocs written.\n"
    );
    let empty = env.run(&["excerpts", A, " "]);
    assert_eq!(empty.status.code(), Some(2));
}

#[test]
fn export_keeps_only_the_turns_asked_for() {
    let env = Env::new();
    three_turns(&env);
    let md = |args: &[&str]| {
        let mut all = vec!["export", A];
        all.extend(args);
        String::from_utf8(env.run(&all).stdout).unwrap()
    };
    let middle = md(&["--turns", "2"]);
    assert!(middle.contains("- Turns: 2-2 of 3\n"), "{middle}");
    assert!(middle.contains("## You · turn 2\n\nnow the docs"));
    assert!(middle.contains("Docs written."));
    assert!(!middle.contains("turn 1") && !middle.contains("turn 3"));
    let tail = md(&["--last-turns", "2"]);
    assert!(tail.contains("- Turns: 2-3 of 3\n") && !tail.contains("turn 1"));
    assert_eq!(md(&["--turns", "2-"]), tail);
    assert!(md(&["--turns", "7-"]).contains("- Turns: none of 3\n"));
    let bad = env.run(&["export", A, "--turns", "3-1"]);
    assert_eq!(bad.status.code(), Some(2));
    let both = env.run(&["export", A, "--turns", "1", "--last-turns", "1"]);
    assert_eq!(both.status.code(), Some(2));
}

#[test]
fn export_writes_prompts_and_replies_with_tools_counted() {
    let env = Env::new();
    env.session(
        A,
        &[
            custom_title(A, "Fix the build"),
            user("why does it fail"),
            assistant("Let me look.", "claude-x"),
            tool_use("Bash"),
            tool_result(),
            tool_use("Bash"),
            assistant("Found it.", "claude-x"),
            user("<system-reminder>injected</system-reminder>"),
            user("thanks"),
        ],
    );
    // Markdown is the default, on a pipe too.
    let printed = env.run(&["export", A]);
    assert!(printed.status.success());
    let md = String::from_utf8(printed.stdout).unwrap();
    assert!(md.starts_with("# Fix the build\n"), "{md}");
    assert!(md.contains("\n## You · turn 1\n\nwhy does it fail\n"));
    assert!(md.contains("\n## Claude\n\nLet me look.\n\nFound it.\n\n*Tools: Bash ×2*\n"));
    assert!(md.contains("\n## You · turn 2\n\nthanks\n"));
    assert!(!md.contains("injected"));
    assert!(!md.contains("- Turns:"), "a whole export names no range");

    // --output-file receives exactly what stdout would have, and stdout nothing.
    let file = env.path("exports/new/fix.md");
    let out = env.run(&["export", A, "--output-file", file.to_str().unwrap()]);
    assert!(out.status.success() && out.stdout.is_empty(), "{out:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), md);

    let doc = env.ok(&["export", A]);
    assert_eq!(doc, json!({"id": A, "markdown": md, "truncated": false}));
    let json_file = env.path("exports/doc.json");
    let out = env.run(&[
        "export",
        A,
        "--json",
        "--output-file",
        json_file.to_str().unwrap(),
    ]);
    assert!(out.status.success() && out.stdout.is_empty());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&json_file).unwrap()).unwrap(),
        doc
    );
}

#[test]
fn a_long_json_export_is_cut_and_kept_whole_until_delete() {
    let env = Env::new();
    let prompt = "x".repeat(300 * 1024);
    env.session(A, &[user(&prompt)]);
    let doc = env.ok(&["export", A]);
    assert_eq!(doc["truncated"], true);
    assert!(doc["markdown"].as_str().unwrap().len() <= 256 * 1024);
    let kept = PathBuf::from(doc["output_file"].as_str().unwrap());
    let whole = String::from_utf8(env.run(&["export", A]).stdout).unwrap();
    assert_eq!(fs::read_to_string(&kept).unwrap(), whole);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let dry = env.ok(&["delete", A, "--dry-run"]);
    let listed = dry["targets"][0]["paths"].as_array().unwrap();
    assert!(
        listed.iter().any(|p| p.as_str() == kept.to_str()),
        "{listed:?}"
    );
    env.ok(&["delete", A, "--yes"]);
    assert!(!kept.exists());
}

// ---- introspection, formats, errors -----------------------------------------

/// The O4 subset of JSON Schema: type (or [type, "null"]), enum, properties
/// (no other field allowed), required, items.
fn check(value: &Value, schema: &Value, at: &str) -> Result<(), String> {
    if schema == &json!({}) {
        return Ok(());
    }
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().map(|t| t.as_str().unwrap()).collect(),
        other => return Err(format!("{at}: schema without type: {other}")),
    };
    let actual = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_u64() || n.is_i64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let fits = types.contains(&actual) || (actual == "integer" && types.contains(&"number"));
    if !fits {
        return Err(format!("{at}: {actual} is not {types:?}"));
    }
    if let Some(choices) = schema["enum"].as_array()
        && !choices.contains(value)
    {
        return Err(format!("{at}: {value} not in {choices:?}"));
    }
    match value {
        Value::Object(map) => {
            let props = schema["properties"]
                .as_object()
                .ok_or(format!("{at}: no properties"))?;
            for key in schema["required"]
                .as_array()
                .ok_or(format!("{at}: no required"))?
            {
                if !map.contains_key(key.as_str().unwrap()) {
                    return Err(format!("{at}: missing {key}"));
                }
            }
            for (key, v) in map {
                let sub = props
                    .get(key)
                    .ok_or(format!("{at}: undeclared field {key}"))?;
                check(v, sub, &format!("{at}.{key}"))?;
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                check(item, &schema["items"], &format!("{at}[{i}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl Env {
    fn schema(&self, path: &[&str]) -> Value {
        let mut args = vec!["schema"];
        args.extend(path);
        let out = self.run(&args);
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// `ok`, plus the result must match the command's declared output schema.
    fn conforms(&self, command: &str, args: &[&str]) -> Value {
        let mut full = vec![command];
        full.extend(args);
        let v = self.ok(&full);
        let schema = &self.schema(&[command])["output"];
        check(&v, schema, command).unwrap_or_else(|e| panic!("{full:?}: {e}\n{v:#}"));
        v
    }
}

#[test]
fn every_json_result_matches_its_declared_schema() {
    let env = Env::new();
    env.session(
        A,
        &[
            custom_title(A, "Alpha"),
            user("alpha prompt"),
            assistant("reply", "m"),
        ],
    );
    env.session(B, &[user("beta")]);
    env.session(C, &[]);
    env.full_footprint(B);
    env.conforms("list", &["--project", PROJECT]);
    env.conforms("list", &["--project", PROJECT, "--limit", "1"]);
    env.conforms("search", &["alpha", "--project", PROJECT]);
    env.conforms("get", &[A]);
    env.conforms("export", &[A]);
    env.conforms("rename", &[A, "New"]);
    env.conforms("rename", &[A, "New"]);
    env.conforms("pin", &[A]);
    env.conforms("unpin", &[A]);
    env.conforms("archive", &[B]);
    env.conforms("archive", &[B]);
    env.conforms("restore", &[B]);
    env.conforms("restore", &[B]);
    env.conforms("delete", &[B, "--dry-run"]);
    env.conforms("delete", &[B, "--yes"]);
    env.conforms("delete-empty", &["--project", PROJECT, "--dry-run"]);
    env.conforms("delete-empty", &["--project", PROJECT, "--yes"]);
    env.conforms("delete-empty", &["--project", PROJECT]);
}

#[test]
fn schema_index_lists_every_command_with_its_contract() {
    let env = Env::new();
    let index = env.schema(&[]);
    assert_eq!(index["schema_version"], "1");
    assert_eq!(index["tool_version"], env!("CARGO_PKG_VERSION"));
    assert!(index.get("conformance").is_none(), "no claim is made");
    assert_eq!(
        index["format_defaults"],
        json!({"tty": "text", "non_tty": "json"})
    );
    for code in ["0", "1", "2"] {
        assert!(index["exit_codes"][code].is_string());
    }
    let globals: Vec<&str> = index["global_flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(globals, vec!["json", "project", "current"]);
    let names: Vec<&str> = index["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "sorted by name");
    assert_eq!(
        names,
        vec![
            "archive",
            "delete",
            "delete-empty",
            "excerpts",
            "export",
            "get",
            "list",
            "pin",
            "rename",
            "restore",
            "search",
            "unpin"
        ]
    );
    for entry in index["commands"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let detail = env.schema(&[name]);
        assert_eq!(detail["name"], name);
        assert_eq!(detail["effects"], entry["effects"]);
        for field in ["args", "flags"] {
            for input in detail[field].as_array().unwrap() {
                assert!(
                    !input["description"].as_str().unwrap().is_empty(),
                    "{name}: {input}"
                );
                assert!(
                    !globals.contains(&input["name"].as_str().unwrap()),
                    "{name} repeats a global"
                );
            }
        }
        let output = &detail["output"];
        assert_eq!(output["type"], "object", "{name}");
        if detail["effects"] != "read_only" {
            assert_eq!(output["properties"]["changed"]["type"], "boolean", "{name}");
            assert!(
                output["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("changed"))
            );
        }
        // --help names every flag the detail declares.
        let help = String::from_utf8(env.run(&[name, "--help"]).stdout).unwrap();
        for flag in detail["flags"].as_array().unwrap() {
            assert!(
                help.contains(&format!("--{}", flag["name"].as_str().unwrap())),
                "{name}"
            );
        }
    }
    let confirming: Vec<&str> = index["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .filter(|n| env.schema(&[n])["confirm"] == true)
        .collect();
    assert_eq!(confirming, vec!["delete", "delete-empty"]);
    assert_eq!(
        env.schema(&["export"])["format_defaults"],
        json!({"tty": "markdown", "non_tty": "markdown"})
    );
    assert_eq!(
        env.schema(&["list"])["flags"][2],
        json!({
            "name": "limit", "description": "Return at most N sessions; has_more tells whether more matched",
            "type": "integer", "required": false, "default": 50
        })
    );
    assert_eq!(env.schema(&["rename"])["args"][1]["variadic"], true);
}

#[test]
fn schema_needs_no_config_and_rejects_unknown_paths() {
    let out = Command::new(env!("CARGO_BIN_EXE_sessile"))
        .args(["schema", "--json", "--project", "/nowhere"])
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let index: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(index["commands"].is_array());

    let env = Env::new();
    for path in [
        vec!["schema", "nope"],
        vec!["schema", "list", "extra"],
        vec!["schema", "schema"],
    ] {
        let out = env.run(&path);
        assert_eq!(out.status.code(), Some(2), "{path:?}");
        assert!(out.stdout.is_empty());
        assert_eq!(
            last_line_json(&out.stderr)["error"]["kind"],
            "invalid_input"
        );
    }
}

#[test]
fn version_prints_the_bare_version() {
    let out = Env::new().run(&["--version"]);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn json_is_the_default_off_a_terminal_and_errors_are_objects() {
    let env = Env::new();
    env.session(A, &[user("a")]);
    // The test harness pipes stdout and stderr: no --json needed for either.
    let out = env.run(&["list", "--project", PROJECT]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["items"][0]["id"], A);
    let out = env.run(&["get", B]);
    assert_eq!(out.status.code(), Some(1));
    let err = last_line_json(&out.stderr);
    assert_eq!(err["error"]["kind"], "not_found");
    assert!(err["error"]["hint"].is_string());
    assert_eq!(err.as_object().unwrap().len(), 1, "one top-level field");
    // Unknown flags and bad values are usage errors, exit 2.
    for args in [
        vec!["list", "--bogus"],
        vec!["list", "--limit", "x"],
        vec!["frobnicate"],
    ] {
        let out = env.run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            last_line_json(&out.stderr)["error"]["kind"],
            "invalid_input"
        );
    }
    // `--` ends the flags: a title may start with a dash.
    let out = env.run(&["rename", A, "--json", "--", "--yes"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["title"], "--yes");
}

#[test]
fn plain_lists_one_escaped_line_per_session() {
    let env = Env::new();
    env.session(A, &[user("a"), custom_title(A, "tab\there\u{1b}[2J")]);
    env.session(B, &[user("b")]);
    let out = env.run(&["list", "--project", PROJECT, "--plain"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines.contains(&format!("{A}\ttab\\there\\u{{1b}}[2J").as_str()),
        "{text}"
    );
    assert!(!text.contains('\u{1b}'), "ESC never reaches the terminal");
    // A short page says so on stderr, since plain has no room for has_more.
    let out = env.run(&["list", "--project", PROJECT, "--plain", "--limit", "1"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 1);
    assert!(String::from_utf8(out.stderr).unwrap().contains("--limit"));
    assert_eq!(
        env.fails(&["list", "--project", PROJECT, "--plain"], 2)["kind"],
        "invalid_input",
        "--plain with --json"
    );
}

#[cfg(unix)]
#[test]
fn a_closed_pipe_ends_quietly_with_exit_0() {
    use std::io::Read;
    use std::process::Stdio;
    let env = Env::new();
    env.session(A, &[user(&"y".repeat(1 << 20))]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_sessile"))
        .args(["export", A])
        .env("CLAUDE_CONFIG_DIR", env.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = [0u8; 16];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut first)
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_transcript_is_reported_never_guessed() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    env.session(A, &[user("fine")]);
    env.session(B, &[]);
    let locked = env.path(&format!("projects/{SLUG}/{B}.jsonl"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&locked).is_ok() {
        return; // root reads anything; nothing to test
    }
    // The page of what was read still arrives, marked partial, and the call fails.
    for (command, args) in [("list", vec![]), ("search", vec!["fine"])] {
        let mut full = vec![command];
        full.extend(&args);
        full.extend(["--project", PROJECT, "--json"]);
        let out = env.run(&full);
        assert_eq!(out.status.code(), Some(1), "{command}: {out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        check(&v, &env.schema(&[command])["output"], command).unwrap();
        assert_eq!(ids_of(v["items"].as_array().unwrap()), vec![A]);
        assert_eq!(v["partial"], true);
        let locked_name = format!("{B}.jsonl");
        assert!(
            v["unreadable"][0]["path"]
                .as_str()
                .unwrap()
                .ends_with(&locked_name)
        );
        let error = &last_line_json(&out.stderr)["error"];
        assert_eq!(error["kind"], "io_error");
        assert!(
            error["context"]["paths"][0]
                .as_str()
                .unwrap()
                .ends_with(&locked_name)
        );
    }
    // Its prompt count is unknown, so delete-empty leaves it alone.
    let done = env.conforms("delete-empty", &["--project", PROJECT, "--yes"]);
    assert_eq!(done["targets"], json!([]));
    assert_eq!(done["skipped"][0]["id"], B);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
}
