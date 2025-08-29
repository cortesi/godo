use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Helper struct to manage test environments
struct TestEnv {
    _temp_dir: TempDir,
    repo_dir: PathBuf,
    godo_dir: PathBuf,
    godo_binary: PathBuf,
}

impl TestEnv {
    /// Create a new test environment with a git repository
    fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let repo_dir = temp_dir.path().join("test-repo");
        let godo_dir = temp_dir.path().join("godo");

        // Create directories
        fs::create_dir(&repo_dir)?;
        fs::create_dir(&godo_dir)?;

        // Initialize git repository
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["init"])
            .output()?;

        // Configure git user
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.email", "test@example.com"])
            .output()?;

        Command::new("git")
            .current_dir(&repo_dir)
            .args(["config", "user.name", "Test User"])
            .output()?;

        // Create initial commit
        fs::write(repo_dir.join("README.md"), "# Test Project")?;
        Command::new("git")
            .current_dir(&repo_dir)
            .args(["add", "README.md"])
            .output()?;

        Command::new("git")
            .current_dir(&repo_dir)
            .args(["commit", "-m", "Initial commit"])
            .output()?;

        let godo_binary = PathBuf::from(env!("CARGO_BIN_EXE_godo"));

        Ok(Self {
            _temp_dir: temp_dir,
            repo_dir,
            godo_dir,
            godo_binary,
        })
    }

    /// Run a godo command and return the output
    fn run_godo(&self, args: &[&str]) -> Result<Output> {
        let mut cmd_args = vec![
            "--dir",
            self.godo_dir.to_str().unwrap(),
            "--no-prompt", // Always skip prompts in tests
        ];
        cmd_args.extend_from_slice(args);

        Ok(Command::new(&self.godo_binary)
            .current_dir(&self.repo_dir)
            .args(&cmd_args)
            .output()?)
    }

    /// Create a sandbox with a command and return the output
    fn run_in_sandbox(&self, sandbox_name: &str, command: &[&str]) -> Result<Output> {
        let mut args = vec!["run", "--keep", sandbox_name];
        args.extend_from_slice(command);
        self.run_godo(&args)
    }

    /// Remove a sandbox
    fn remove_sandbox(&self, sandbox_name: &str) -> Result<Output> {
        self.run_godo(&["remove", "--force", sandbox_name])
    }
}

#[test]
fn test_exit_code_success() -> Result<()> {
    let env = TestEnv::new()?;

    // Test successful command (exit 0)
    let output = env.run_in_sandbox("test-success", &["true"])?;
    assert!(output.status.success());
    assert_eq!(output.status.code(), Some(0));

    // Cleanup
    let _ = env.remove_sandbox("test-success");

    Ok(())
}

#[test]
fn test_exit_code_failure() -> Result<()> {
    let env = TestEnv::new()?;

    // Test failed command (exit 1)
    let output = env.run_in_sandbox("test-fail", &["false"])?;
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    // Cleanup
    let _ = env.remove_sandbox("test-fail");

    Ok(())
}

#[test]
fn test_exit_code_custom() -> Result<()> {
    let env = TestEnv::new()?;

    // Test custom exit codes using direct exit commands
    let test_cases = vec![
        (2, "test-exit-2"),
        (42, "test-exit-42"),
        (255, "test-exit-255"),
    ];

    for (exit_code, sandbox_name) in test_cases {
        // Use a custom shell script to handle the exit
        let exit_script = format!(
            r#"
#!/bin/sh
exit {}
"#,
            exit_code
        );

        // Write script to repo first
        let script_name = format!("exit_{}.sh", exit_code);
        let script_path = env.repo_dir.join(&script_name);
        fs::write(&script_path, &exit_script)?;

        // Make executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms)?;
        }

        // Now run the script in a sandbox
        let output = env.run_in_sandbox(sandbox_name, &[&format!("./{}", script_name)])?;

        assert!(!output.status.success());
        assert_eq!(
            output.status.code(),
            Some(exit_code),
            "Expected exit code {} for sandbox {}",
            exit_code,
            sandbox_name
        );

        // Cleanup
        let _ = env.remove_sandbox(sandbox_name);
        fs::remove_file(&script_path)?;
    }

    Ok(())
}

#[test]
fn test_exit_code_command_not_found() -> Result<()> {
    let env = TestEnv::new()?;

    // Test command not found (typically exits with 127)
    let output = env.run_in_sandbox("test-not-found", &["this_command_does_not_exist"])?;
    assert!(!output.status.success());
    // Most shells return 127 for command not found
    assert_eq!(output.status.code(), Some(127));

    // Cleanup
    let _ = env.remove_sandbox("test-not-found");

    Ok(())
}

#[test]
fn test_exit_code_with_commit_success() -> Result<()> {
    let env = TestEnv::new()?;

    // Test successful command with --commit
    let output = env.run_godo(&[
        "run",
        "--commit",
        "Test commit",
        "test-commit-success",
        "touch",
        "newfile.txt",
    ])?;

    assert!(output.status.success());
    assert_eq!(output.status.code(), Some(0));

    Ok(())
}

#[test]
fn test_exit_code_with_commit_failure() -> Result<()> {
    let env = TestEnv::new()?;

    // Create a script that creates a file and exits with code 5
    let script_content = r#"
#!/bin/sh
touch newfile.txt
exit 5
"#;
    let script_path = env.repo_dir.join("fail_script.sh");
    fs::write(&script_path, script_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    // Test failed command with --commit (should not commit)
    let output = env.run_godo(&[
        "run",
        "--commit",
        "Test commit",
        "test-commit-fail",
        "./fail_script.sh",
    ])?;

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(5));

    // Verify that the sandbox still exists (wasn't cleaned up due to failure)
    let list_output = env.run_godo(&["list"])?;
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_str.contains("test-commit-fail"));

    // Cleanup
    let _ = env.remove_sandbox("test-commit-fail");
    fs::remove_file(&script_path)?;

    Ok(())
}

#[test]
fn test_argument_quoting_preserved() -> Result<()> {
    let env = TestEnv::new()?;

    // Create a script that verifies its first two args
    let script_content = r#"#!/bin/sh
if [ "$1" = "a b" ] && [ "$2" = "c" ]; then
  exit 0
else
  exit 1
fi
"#;
    let script_path = env.repo_dir.join("args_test.sh");
    fs::write(&script_path, script_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    let output = env.run_in_sandbox("test-args", &["./args_test.sh", "a b", "c"])?;

    assert!(output.status.success());
    assert_eq!(output.status.code(), Some(0));

    // Cleanup
    let _ = env.remove_sandbox("test-args");
    fs::remove_file(&script_path)?;

    Ok(())
}

#[test]
fn test_exit_code_signal_termination() -> Result<()> {
    let env = TestEnv::new()?;

    // Test signal termination (if supported by the platform)
    // This test might behave differently on different platforms
    if cfg!(unix) {
        let script_content = r#"
#!/bin/sh
# Send SIGTERM to self
kill $$
"#;
        let script_path = env.repo_dir.join("signal_test.sh");
        fs::write(&script_path, script_content)?;

        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;

        let output = env.run_in_sandbox("test-signal", &["./signal_test.sh"])?;
        assert!(!output.status.success());
        // The exact exit code for signal termination can vary
        // On Unix, it's typically 128 + signal number (143 for SIGTERM)
        // But we'll just check that it's non-zero
        assert_ne!(output.status.code(), Some(0));

        // Cleanup
        let _ = env.remove_sandbox("test-signal");
        fs::remove_file(&script_path)?;
    }

    Ok(())
}

#[test]
fn test_multiple_commands_exit_codes() -> Result<()> {
    let env = TestEnv::new()?;

    // Create a script that runs multiple commands
    let script_content = r#"
#!/bin/sh
true
false
true
exit 3
"#;
    let script_path = env.repo_dir.join("multi_cmd.sh");
    fs::write(&script_path, script_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    // Test that the script's exit code is returned
    let output = env.run_in_sandbox("test-multiple", &["./multi_cmd.sh"])?;

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));

    // Cleanup
    let _ = env.remove_sandbox("test-multiple");
    fs::remove_file(&script_path)?;

    Ok(())
}

#[test]
fn test_sandbox_reuse_exit_code() -> Result<()> {
    let env = TestEnv::new()?;

    // Create sandbox with successful command
    let output1 = env.run_in_sandbox("test-reuse", &["true"])?;
    assert_eq!(output1.status.code(), Some(0));

    // Reuse sandbox with failing command
    let output2 = env.run_in_sandbox("test-reuse", &["false"])?;
    assert_eq!(output2.status.code(), Some(1));

    // Cleanup
    let _ = env.remove_sandbox("test-reuse");

    Ok(())
}

#[test]
fn test_cleanup_sandbox_removes_merged_branch_without_worktree() -> Result<()> {
    let env = TestEnv::new()?;

    // Create a sandbox and make a commit
    env.run_in_sandbox("test-cleanup", &["touch", "file.txt"])?;

    // Add and commit the file in the sandbox
    let sandbox_path = env.godo_dir.join("test-repo").join("test-cleanup");
    Command::new("git")
        .current_dir(&sandbox_path)
        .args(["add", "file.txt"])
        .output()?;
    Command::new("git")
        .current_dir(&sandbox_path)
        .args(["commit", "-m", "Add file"])
        .output()?;

    // Merge the branch back to main/master
    Command::new("git")
        .current_dir(&env.repo_dir)
        .args(["merge", "godo/test-cleanup"])
        .output()?;

    // Remove just the worktree, keeping the branch
    Command::new("git")
        .current_dir(&env.repo_dir)
        .args(["worktree", "remove", sandbox_path.to_str().unwrap()])
        .output()?;

    // Verify branch still exists
    let branches_output = Command::new("git")
        .current_dir(&env.repo_dir)
        .args(["branch", "--list", "godo/test-cleanup"])
        .output()?;
    let branches_str = String::from_utf8_lossy(&branches_output.stdout);
    assert!(
        branches_str.contains("godo/test-cleanup"),
        "Branch should exist before cleanup"
    );

    // Run clean command
    let clean_output = env.run_godo(&["clean", "test-cleanup"])?;
    if !clean_output.status.success() {
        eprintln!("Clean command failed:");
        eprintln!("stdout: {}", String::from_utf8_lossy(&clean_output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&clean_output.stderr));
    }
    assert!(clean_output.status.success());

    // Verify branch was removed
    let branches_after_output = Command::new("git")
        .current_dir(&env.repo_dir)
        .args(["branch", "--list", "godo/test-cleanup"])
        .output()?;
    let branches_after_str = String::from_utf8_lossy(&branches_after_output.stdout);
    assert!(
        !branches_after_str.contains("godo/test-cleanup"),
        "Branch should be removed after cleanup"
    );

    Ok(())
}
