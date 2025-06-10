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
