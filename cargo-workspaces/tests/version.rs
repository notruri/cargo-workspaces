mod utils;

use assert_cmd::Command;
use serial_test::serial;
use tempfile::TempDir;

use std::{
    fs::{copy, create_dir_all, read_dir, read_to_string},
    path::Path,
    process::Command as StdCommand,
};

#[test]
#[serial]
fn test_sub_root_tags() {
    let dir = setup_fixture("../fixtures/sub_root", &["nested"]);

    run_version(dir.path());

    let root_paths = git_head_paths(dir.path());
    let root_tags = git_tags(dir.path());
    let root_message = git_head_message(dir.path());
    let nested_tags = git_tags(&dir.path().join("nested"));
    let nested_subject = git_head_subject(&dir.path().join("nested"));

    assert!(root_paths.contains(&"nested".to_string()));
    assert_eq!(root_tags, vec!["sub-root-member@0.1.1", "v0.1.1"]);
    assert_eq!(nested_tags, vec!["v0.1.1"]);
    assert!(root_message.contains("External Packages:\nsub-root-nested@0.1.1"));
    assert_eq!(nested_subject, "Release v0.1.1");
}

#[test]
#[serial]
fn test_sub_root_amend_includes_submodule_pointer() {
    let dir = setup_fixture("../fixtures/sub_root", &["nested"]);

    run_version_with_args(dir.path(), &["--amend"]);

    let root_paths = git_head_paths(dir.path());

    assert_eq!(git_commit_count(dir.path()), 1);
    assert!(root_paths.contains(&"nested".to_string()));
}

#[test]
#[serial]
fn test_sub_virtual_tags() {
    let dir = setup_fixture("../fixtures/sub_virtual", &["nested"]);

    run_version(dir.path());

    let root_tags = git_tags(dir.path());
    let root_message = git_head_message(dir.path());
    let nested_tags = git_tags(&dir.path().join("nested"));
    let nested_subject = git_head_subject(&dir.path().join("nested"));

    assert_eq!(root_tags, vec!["sub-virtual-member@0.1.1", "v0.1.1"]);
    assert_eq!(nested_tags, vec!["v0.1.1"]);
    assert!(root_message.contains("External Packages:\nsub-virtual-nested@0.1.1"));
    assert_eq!(nested_subject, "Release v0.1.1");
}

#[test]
#[serial]
fn test_non_independent_separate_repo_uses_repo_tag_label() {
    let dir = setup_fixture("../fixtures/sub_virtual_nonind", &["nested"]);

    run_version(dir.path());

    let root_tags = git_tags(dir.path());
    let nested_tags = git_tags(&dir.path().join("nested"));
    let nested_subject = git_head_subject(&dir.path().join("nested"));

    assert_eq!(root_tags, vec!["sub-virtual-nonind-member@0.1.1", "v0.1.1"]);
    assert_eq!(nested_tags, vec!["v0.1.1"]);
    assert_eq!(nested_subject, "Release v0.1.1");
}

#[test]
#[serial]
fn test_workspace_dependencies_are_updated_for_forced_independent_bump() {
    let dir = setup_fixture("../fixtures/sub_virtual_wsdeps", &["nested"]);

    run_version_with_args(dir.path(), &["--force", "sub-virtual-wsdeps-nested"]);
    let root_message = git_head_message(dir.path());

    assert!(read_to_string(dir.path().join("Cargo.toml"))
        .expect("read workspace manifest")
        .contains(r#"sub-virtual-wsdeps-nested = { version = "0.1.1", path = "nested" }"#));
    assert!(read_to_string(dir.path().join("member/Cargo.toml"))
        .expect("read member manifest")
        .contains(r#"sub-virtual-wsdeps-nested = { workspace = true }"#));
    assert!(root_message.contains("External Packages:\nsub-virtual-wsdeps-nested@0.1.1"));
}

#[test]
#[serial]
fn test_root_commit_is_not_created_when_root_has_no_changes() {
    let dir = setup_fixture("../fixtures/sub_only", &["nested"]);

    run_version(dir.path());

    let status = git_status(dir.path());

    assert_eq!(git_commit_count(dir.path()), 1);
    assert!(status.contains(&"M nested".to_string()));
    assert_eq!(git_tags(dir.path()), Vec::<String>::new());
    assert_eq!(
        git_head_subject(&dir.path().join("nested")),
        "Release v0.1.1"
    );
}

#[test]
#[serial]
fn test_root_tracking_commit_is_optional_for_external_only_releases() {
    let dir = setup_fixture("../fixtures/sub_only", &["nested"]);

    run_version_with_args(dir.path(), &["--root-tracking-commit"]);

    let root_paths = git_head_paths(dir.path());
    let root_message = git_head_message(dir.path());

    assert_eq!(git_commit_count(dir.path()), 2);
    assert_eq!(git_head_subject(dir.path()), "Track external releases");
    assert!(root_paths.contains(&"nested".to_string()));
    assert!(root_message.contains("External Packages:\nsub-only-nested@0.1.1"));
    assert_eq!(git_tags(dir.path()), Vec::<String>::new());
}

#[test]
#[serial]
fn test_root_commit_message_separates_local_and_external_packages() {
    let dir = setup_fixture("../fixtures/sub_virtual_wsdeps", &["nested"]);

    run_version(dir.path());

    let root_message = git_head_message(dir.path());

    assert!(root_message.contains("Local Packages:\nsub-virtual-wsdeps-member@0.1.1"));
    assert!(root_message.contains("External Packages:\nsub-virtual-wsdeps-nested@0.1.1"));
}

fn setup_fixture(src: &str, nested_roots: &[&str]) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    copy_dir(Path::new(src), dir.path());

    for nested_root in nested_roots {
        init_repo(&dir.path().join(nested_root));
    }

    init_repo(dir.path());
    dir
}

fn run_version(dir: &Path) {
    run_version_with_args(dir, &[]);
}

fn run_version_with_args(dir: &Path, extra_args: &[&str]) {
    let mut args = vec![
        "ws",
        "version",
        "patch",
        "-y",
        "--no-git-push",
        "--allow-branch",
        "*",
    ];
    args.extend_from_slice(extra_args);

    let output = Command::new(assert_cmd::cargo::cargo_bin!("cargo-ws"))
        .current_dir(dir)
        .args(&args)
        .output()
        .expect("run cargo-ws version");

    if !output.status.success() {
        panic!(
            "version command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn git_tags(dir: &Path) -> Vec<String> {
    let output = git(dir, &["tag", "--list", "--sort=refname"]);
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn git_head_subject(dir: &Path) -> String {
    git(dir, &["log", "-1", "--pretty=%s"])
}

fn git_head_message(dir: &Path) -> String {
    git(dir, &["log", "-1", "--pretty=%B"])
}

fn git_head_paths(dir: &Path) -> Vec<String> {
    git(dir, &["show", "--pretty=format:", "--name-only", "HEAD"])
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn git_commit_count(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"])
        .parse()
        .expect("git commit count")
}

fn git_status(dir: &Path) -> Vec<String> {
    git(dir, &["status", "--short"])
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn init_repo(dir: &Path) {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test User"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = StdCommand::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");

    if !output.status.success() {
        panic!(
            "git {:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
            args,
            dir.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .expect("utf8 git stdout")
        .trim()
        .to_string()
}

fn copy_dir(src: &Path, dst: &Path) {
    for entry in read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("read fixture entry");
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry.file_type().expect("fixture entry type").is_dir() {
            create_dir_all(&dst_path).expect("create fixture dir");
            copy_dir(&src_path, &dst_path);
        } else {
            copy_file(&src_path, &dst_path);
        }
    }
}

fn copy_file(src: &Path, dst: &Path) {
    if let Some(parent) = dst.parent() {
        create_dir_all(parent).expect("create file parent");
    }

    copy(src, dst).expect("copy fixture file");
}
