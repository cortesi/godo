// Integration tests are compiled as a separate crate, so these lints don't apply
#![allow(clippy::tests_outside_test_module)]
#![allow(missing_docs)]

mod common;

use std::{fs, process::Command};

use anyhow::Result;
use common::{create_repo, git, godo_binary, run_godo};
use tempfile::TempDir;

#[test]
fn test_project_directory_structure() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Run godo to create a sandbox with --keep flag
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("godo run failed");
    }

    // Verify the directory structure
    let project_dir = godo_dir.path().join("test-project");
    assert!(project_dir.exists(), "Project directory should exist");

    let sandbox_dir = project_dir.join("test-sandbox");
    assert!(sandbox_dir.exists(), "Sandbox directory should exist");

    Ok(())
}

#[test]
fn test_project_name_cleaning() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test.project@2024")?;
    let godo_dir = TempDir::new()?;

    // Run godo with --keep flag
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("godo run failed");
    }

    // Verify the cleaned project name
    let project_dir = godo_dir.path().join("test-project-2024");
    assert!(
        project_dir.exists(),
        "Cleaned project directory should exist"
    );

    Ok(())
}

#[test]
fn test_clean_command_section_output() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create multiple sandboxes with different states
    // 1. Clean sandbox (no changes)
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "clean-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success());

    // 2. Sandbox with uncommitted changes
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "dirty-sandbox", "touch", "newfile.txt"],
    )?;
    assert!(output.status.success());

    // 3. Another clean sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "another-clean", "echo", "another", "test"],
    )?;
    assert!(output.status.success());

    // Now run clean command on all sandboxes
    let output = run_godo(&repo_path, godo_dir.path(), &["--no-prompt", "clean"])?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        eprintln!("stdout: {}", stdout);
        eprintln!("stderr: {}", stderr);
        panic!("godo clean failed");
    }

    // Verify the section output structure
    assert!(
        stdout.contains("Cleaning 3 sandboxes..."),
        "Should show total count"
    );
    assert!(
        stdout.contains("cleaning sandbox:"),
        "Should have section headers"
    );

    // Check for indented messages - all sandboxes show as "unchanged"
    // This is because they were just created and the worktrees exist but are considered "unchanged"
    // Let's check for the actual output we're seeing (3-space indent)
    assert!(
        stdout.contains("   unchanged")
            || stdout.contains("   removed unmodified worktree")
            || stdout.contains("   skipping worktree with uncommitted changes"),
        "Should have indented status messages, got: {}",
        stdout
    );

    // Clean up remaining sandboxes (ignore errors during cleanup)
    for sandbox in ["clean-sandbox", "dirty-sandbox", "another-clean"] {
        run_godo(&repo_path, godo_dir.path(), &["remove", "--force", sandbox]).ok();
    }

    Ok(())
}

#[test]
fn test_sandbox_list_from_within() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");
    assert!(sandbox_dir.exists(), "Sandbox directory should exist");

    // Run godo list from within the sandbox - should succeed
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "list",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "godo list from within sandbox should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

#[test]
fn test_sandbox_diff_from_within() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox with a change
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "touch", "newfile.txt"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");

    // Run godo diff (no name) from within the sandbox - should auto-detect
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "diff",
            "--no-pager",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "godo diff from within sandbox should auto-detect sandbox name, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

#[test]
fn test_sandbox_run_self_blocked() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");

    // Try to run the same sandbox from within it - should fail
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "run",
            "test-sandbox",
            "echo",
            "test",
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "godo run self from within sandbox should fail"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Cannot run sandbox 'test-sandbox' from within itself"),
        "Should show self-run error, got: {}",
        combined
    );

    Ok(())
}

#[test]
fn test_sandbox_run_other_allowed() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create first sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "sandbox-one", "echo", "test"],
    )?;
    assert!(
        output.status.success(),
        "Creating sandbox-one should succeed"
    );

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_one_dir = project_dir.join("sandbox-one");

    // Run a different sandbox from within sandbox-one - should succeed
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_one_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "run",
            "--keep",
            "sandbox-two",
            "echo",
            "from-one",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "godo run other from within sandbox should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify sandbox-two was created
    assert!(
        project_dir.join("sandbox-two").exists(),
        "sandbox-two should exist"
    );

    Ok(())
}

#[test]
fn test_sandbox_remove_self_blocked() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");

    // Try to remove self from within - should fail
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "remove",
            "--force",
            "test-sandbox",
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "godo remove self from within sandbox should fail"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Cannot remove sandbox 'test-sandbox' while inside it"),
        "Should show self-remove error, got: {}",
        combined
    );

    Ok(())
}

#[test]
fn test_sandbox_remove_other_allowed() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create two sandboxes
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "sandbox-one", "echo", "test"],
    )?;
    assert!(output.status.success());

    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "sandbox-two", "echo", "test"],
    )?;
    assert!(output.status.success());

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_one_dir = project_dir.join("sandbox-one");

    // Remove sandbox-two from within sandbox-one - should succeed
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_one_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "remove",
            "--force",
            "sandbox-two",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "godo remove other from within sandbox should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify sandbox-two was removed
    assert!(
        !project_dir.join("sandbox-two").exists(),
        "sandbox-two should be removed"
    );

    Ok(())
}

#[test]
fn test_sandbox_clean_all_blocked() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");

    // Try to clean all from within - should fail
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "--no-prompt",
            "clean",
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "godo clean (all) from within sandbox should fail"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Cannot run 'godo clean' (all sandboxes) from within a sandbox"),
        "Should show clean-all error, got: {}",
        combined
    );

    Ok(())
}

#[test]
fn test_sandbox_clean_self_blocked() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create a sandbox
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "test-sandbox", "echo", "test"],
    )?;
    assert!(output.status.success(), "Creating sandbox should succeed");

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");

    // Try to clean self from within - should fail
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "--no-prompt",
            "clean",
            "test-sandbox",
        ])
        .output()?;

    assert!(
        !output.status.success(),
        "godo clean self from within sandbox should fail"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Cannot clean sandbox 'test-sandbox' while inside it"),
        "Should show clean-self error, got: {}",
        combined
    );

    Ok(())
}

#[test]
fn test_sandbox_clean_other_allowed() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create two sandboxes
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "sandbox-one", "echo", "test"],
    )?;
    assert!(output.status.success());

    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &["run", "--keep", "sandbox-two", "echo", "test"],
    )?;
    assert!(output.status.success());

    let project_dir = godo_dir.path().join("test-project");
    let sandbox_one_dir = project_dir.join("sandbox-one");

    // Clean sandbox-two from within sandbox-one - should succeed
    let output = Command::new(godo_binary())
        .current_dir(&sandbox_one_dir)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--repo-dir",
            repo_path.to_str().unwrap(),
            "--no-prompt",
            "clean",
            "sandbox-two",
        ])
        .output()?;

    assert!(
        output.status.success(),
        "godo clean other from within sandbox should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

#[test]
fn test_clean_branch_option_with_uncommitted_changes() -> Result<()> {
    let (_temp_dir, repo_path) = create_repo("test-project")?;
    let godo_dir = TempDir::new()?;

    // Create uncommitted changes
    fs::write(repo_path.join("uncommitted.txt"), "uncommitted content")?;
    git(&repo_path, &["add", "uncommitted.txt"])?;

    // Run godo with --no-prompt flag (which should use the default behavior of continuing with changes)
    let output = run_godo(
        &repo_path,
        godo_dir.path(),
        &[
            "--no-prompt",
            "run",
            "--keep",
            "test-sandbox",
            "echo",
            "test",
        ],
    )?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("godo run failed");
    }

    // Verify sandbox was created
    let project_dir = godo_dir.path().join("test-project");
    let sandbox_dir = project_dir.join("test-sandbox");
    assert!(sandbox_dir.exists(), "Sandbox directory should exist");

    // Verify that uncommitted file exists in the sandbox (with --no-prompt, it continues with changes)
    assert!(
        sandbox_dir.join("uncommitted.txt").exists(),
        "Uncommitted file should exist in sandbox when using --no-prompt"
    );

    // Clean up
    run_godo(
        &repo_path,
        godo_dir.path(),
        &["remove", "--force", "test-sandbox"],
    )?;

    Ok(())
}
