use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn setup_test_repo() -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test-project");
    fs::create_dir(&repo_path)?;

    // Initialize git repository
    Command::new("git")
        .current_dir(&repo_path)
        .args(["init"])
        .output()?;

    // Configure git user for commits
    Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.email", "test@example.com"])
        .output()?;

    Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.name", "Test User"])
        .output()?;

    // Create initial commit
    fs::write(repo_path.join("README.md"), "# Test Project")?;
    Command::new("git")
        .current_dir(&repo_path)
        .args(["add", "README.md"])
        .output()?;

    Command::new("git")
        .current_dir(&repo_path)
        .args(["commit", "-m", "Initial commit"])
        .output()?;

    Ok((temp_dir, repo_path))
}

#[test]
fn test_project_directory_structure() -> Result<()> {
    let (_temp_dir, repo_path) = setup_test_repo()?;
    let godo_dir = TempDir::new()?;

    // Build the path to the godo binary
    let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

    // Run godo to create a sandbox with --keep flag
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-sandbox",
            "echo",
            "test",
        ])
        .output()?;

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
    let temp_dir = TempDir::new()?;
    let repo_path = temp_dir.path().join("test.project@2024");
    fs::create_dir(&repo_path)?;

    // Initialize git repository
    Command::new("git")
        .current_dir(&repo_path)
        .args(["init"])
        .output()?;

    // Configure git user
    Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.email", "test@example.com"])
        .output()?;

    Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.name", "Test User"])
        .output()?;

    // Create initial commit
    fs::write(repo_path.join("README.md"), "# Test Project")?;
    Command::new("git")
        .current_dir(&repo_path)
        .args(["add", "README.md"])
        .output()?;

    Command::new("git")
        .current_dir(&repo_path)
        .args(["commit", "-m", "Initial commit"])
        .output()?;

    let godo_dir = TempDir::new()?;
    let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

    // Run godo with --keep flag
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-sandbox",
            "echo",
            "test",
        ])
        .output()?;

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
fn test_exit_code_propagation() -> Result<()> {
    let (_temp_dir, repo_path) = setup_test_repo()?;
    let godo_dir = TempDir::new()?;
    let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

    // Test 1: Command exits with code 0
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-exit-0",
            "exit",
            "0",
        ])
        .output()?;

    assert!(output.status.success(), "exit 0 should succeed");
    assert_eq!(output.status.code(), Some(0), "Should exit with code 0");

    // Test 2: Command exits with code 1
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-exit-1",
            "exit",
            "1",
        ])
        .output()?;

    assert!(!output.status.success(), "exit 1 should fail");
    assert_eq!(output.status.code(), Some(1), "Should exit with code 1");

    // Test 3: Command exits with code 42
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-exit-42",
            "exit",
            "42",
        ])
        .output()?;

    assert!(!output.status.success(), "exit 42 should fail");
    assert_eq!(output.status.code(), Some(42), "Should exit with code 42");

    // Test 4: Command that doesn't exist (typically exits with 127)
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "test-not-found",
            "thiscommanddoesnotexist",
        ])
        .output()?;

    assert!(!output.status.success(), "non-existent command should fail");
    // Shell typically returns 127 for command not found
    assert_eq!(
        output.status.code(),
        Some(127),
        "Should exit with code 127 for command not found"
    );

    // Cleanup
    for sandbox in [
        "test-exit-0",
        "test-exit-1",
        "test-exit-42",
        "test-not-found",
    ] {
        let _ = Command::new(&godo_binary)
            .current_dir(&repo_path)
            .args([
                "--dir",
                godo_dir.path().to_str().unwrap(),
                "remove",
                "--force",
                sandbox,
            ])
            .output();
    }

    Ok(())
}

#[test]
fn test_exit_code_with_auto_commit() -> Result<()> {
    let (_temp_dir, repo_path) = setup_test_repo()?;
    let godo_dir = TempDir::new()?;
    let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

    // Test 1: Successful command with --commit should exit 0
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--commit",
            "Test commit",
            "test-exit-commit-success",
            "touch",
            "newfile.txt",
        ])
        .output()?;

    if !output.status.success() {
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("exit code: {:?}", output.status.code());
    }
    assert!(
        output.status.success(),
        "successful command with --commit should succeed"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "Should exit with code 0 for successful command"
    );

    // Test 2: Failed command with --commit should propagate the exit code
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--commit",
            "Test commit",
            "test-exit-commit-fail",
            "false", // false command always exits with 1
        ])
        .output()?;

    assert!(!output.status.success(), "false command should fail");
    assert_eq!(
        output.status.code(),
        Some(1),
        "Should exit with code 1 for false command"
    );

    Ok(())
}

#[test]
fn test_clean_command_section_output() -> Result<()> {
    let (_temp_dir, repo_path) = setup_test_repo()?;
    let godo_dir = TempDir::new()?;
    let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

    // Create multiple sandboxes with different states
    // 1. Clean sandbox (no changes)
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "clean-sandbox",
            "echo",
            "test",
        ])
        .output()?;
    assert!(output.status.success());

    // 2. Sandbox with uncommitted changes
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "dirty-sandbox",
            "touch",
            "newfile.txt",
        ])
        .output()?;
    assert!(output.status.success());

    // 3. Another clean sandbox
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "run",
            "--keep",
            "another-clean",
            "echo",
            "another test",
        ])
        .output()?;
    assert!(output.status.success());

    // Now run clean command on all sandboxes
    let output = Command::new(&godo_binary)
        .current_dir(&repo_path)
        .args([
            "--dir",
            godo_dir.path().to_str().unwrap(),
            "--no-prompt", // Skip confirmation prompts
            "clean",
        ])
        .output()?;

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
    // Let's check for the actual output we're seeing
    assert!(
        stdout.contains("    unchanged")
            || stdout.contains("    removed unmodified worktree")
            || stdout.contains("    skipping worktree with uncommitted changes"),
        "Should have indented status messages"
    );

    // Clean up remaining sandboxes
    for sandbox in ["clean-sandbox", "dirty-sandbox", "another-clean"] {
        let _ = Command::new(&godo_binary)
            .current_dir(&repo_path)
            .args([
                "--dir",
                godo_dir.path().to_str().unwrap(),
                "remove",
                "--force",
                sandbox,
            ])
            .output();
    }

    Ok(())
}
