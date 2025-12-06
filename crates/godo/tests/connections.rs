use std::{
    fs,
    path::Path,
    process::{Child, Command},
    thread,
    time::Duration,
};

use tempfile::TempDir;

fn godo_binary() -> &'static str {
    env!("CARGO_BIN_EXE_godo")
}

fn git(repo_path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(args)
        .output()
        .expect("failed to run git");

    if !output.status.success() {
        panic!(
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn init_repo(path: &Path) {
    fs::create_dir_all(path).expect("create repo dir");
    git(path, &["init"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Tester"]);
    fs::write(path.join("README.md"), "test").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-m", "init"]);
}

fn spawn_run(repo_dir: &Path, godo_dir: &Path, name: &str, seconds: u64) -> Child {
    Command::new(godo_binary())
        .current_dir(repo_dir)
        .args([
            "--dir",
            godo_dir.to_str().unwrap(),
            "--no-prompt",
            "run",
            "--keep",
            name,
            "sleep",
            &seconds.to_string(),
        ])
        .spawn()
        .expect("spawn godo run")
}

fn list_output(repo_dir: &Path, godo_dir: &Path) -> String {
    let out = Command::new(godo_binary())
        .current_dir(repo_dir)
        .args(["--dir", godo_dir.to_str().unwrap(), "list"])
        .output()
        .expect("run godo list");

    assert!(out.status.success(), "list failed: {:?}", out);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn concurrent_sessions_update_connection_counts() {
    let temp = TempDir::new().unwrap();
    let repo_dir = temp.path().join("repo");
    let godo_dir = temp.path().join("godo");
    init_repo(&repo_dir);

    // Two overlapping runs in the same sandbox.
    // Use short sleep times - just enough to ensure overlap.
    let mut first = spawn_run(&repo_dir, &godo_dir, "shared", 1);
    // Ensure the worktree exists before starting the second.
    thread::sleep(Duration::from_millis(200));
    let mut second = spawn_run(&repo_dir, &godo_dir, "shared", 2);

    // Both active => 2 active connections
    thread::sleep(Duration::from_millis(200));
    let out = list_output(&repo_dir, &godo_dir);
    assert!(
        out.contains("shared") && out.contains("2 active connections"),
        "list while both running: {out}"
    );

    // After the shorter run exits => 1 active connection
    thread::sleep(Duration::from_millis(800));
    let out = list_output(&repo_dir, &godo_dir);
    assert!(
        out.contains("shared") && out.contains("1 active connection"),
        "list with one running: {out}"
    );

    // Let the final run finish; connections label should disappear (0)
    let _ = first.wait();
    let _ = second.wait();

    let out = list_output(&repo_dir, &godo_dir);
    assert!(
        out.contains("shared") && !out.contains("active connection"),
        "list after all exits: {out}"
    );
}
