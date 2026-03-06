use crate::utils::{
    debug, info, validate_value_containing_name, Error, WorkspaceConfig, INTERNAL_ERR,
};

use camino::Utf8PathBuf;
use clap::Parser;
use globset::Glob;
use semver::Version;

use std::{
    collections::BTreeMap as Map,
    process::{Command, ExitStatus},
};

#[derive(Debug, Clone)]
pub struct RepoVersion {
    pub version: Version,
    pub independent: bool,
    pub root: bool,
}

pub fn git(root: &Utf8PathBuf, args: &[&str]) -> Result<(ExitStatus, String, String), Error> {
    debug!("git", args.to_vec().join(" "));

    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|err| Error::Git {
            err,
            args: args.iter().map(|x| x.to_string()).collect(),
        })?;

    Ok((
        output.status,
        String::from_utf8(output.stdout)?.trim().to_owned(),
        String::from_utf8(output.stderr)?.trim().to_owned(),
    ))
}

pub fn git_repository_root(path: &Utf8PathBuf) -> Result<Utf8PathBuf, Error> {
    let (_, out, err) = git(path, &["rev-parse", "--show-toplevel"])?;

    if err.contains("not a git repository") {
        return Err(Error::NotGit);
    }

    Ok(Utf8PathBuf::from(out))
}

#[derive(Debug, Parser)]
#[clap(next_help_heading = "GIT OPTIONS")]
pub struct GitOpt {
    /// Do not commit version changes
    #[clap(long, conflicts_with_all = &[
        "allow-branch", "amend", "message", "no-git-tag",
        "tag-prefix", "individual-tag-prefix", "no-individual-tags",
        "no-git-push", "git-remote", "no-global-tag"
    ])]
    pub no_git_commit: bool,

    /// Specify which branches to allow from [default: master]
    #[clap(long, value_name = "PATTERN", forbid_empty_values(true))]
    pub allow_branch: Option<String>,

    /// Amend the existing commit, instead of generating a new one
    #[clap(long)]
    pub amend: bool,

    /// Use a custom commit message when creating the version commit [default: Release %v]
    #[clap(
        short,
        long,
        conflicts_with_all = &["amend"],
        forbid_empty_values(true)
    )]
    pub message: Option<String>,

    /// Do not tag generated commit
    #[clap(long, conflicts_with_all = &["tag-prefix", "individual-tag-prefix", "no-individual-tags"])]
    pub no_git_tag: bool,

    /// Do not tag individual versions for crates
    #[clap(long, conflicts_with_all = &["individual-tag-prefix"])]
    pub no_individual_tags: bool,

    /// Do not create a global tag for a workspace
    #[clap(long)]
    pub no_global_tag: bool,

    /// Customize tag prefix (can be empty)
    #[clap(long, default_value = "v", value_name = "PREFIX")]
    pub tag_prefix: String,

    /// Customize prefix for individual tags (should contain `%n`)
    #[clap(
        long,
        default_value = "%n@",
        value_name = "PREFIX",
        validator = validate_value_containing_name,
        forbid_empty_values(true)
    )]
    pub individual_tag_prefix: String,

    /// Do not push generated commit and tags to git remote
    #[clap(long, conflicts_with_all = &["git-remote"])]
    pub no_git_push: bool,

    /// Push git changes to the specified remote
    #[clap(
        long,
        default_value = "origin",
        value_name = "REMOTE",
        forbid_empty_values(true)
    )]
    pub git_remote: String,
}

impl GitOpt {
    pub fn validate(
        &self,
        roots: &[Utf8PathBuf],
        config: &WorkspaceConfig,
    ) -> Result<Map<Utf8PathBuf, String>, Error> {
        let mut branches = Map::new();

        if !self.no_git_commit {
            for root in roots {
                let (_, out, err) = git(root, &["rev-list", "--count", "--all", "--max-count=1"])?;

                if err.contains("not a git repository") {
                    return Err(Error::NotGit);
                }

                if out == "0" {
                    return Err(Error::NoCommits);
                }

                let (_, branch, _) = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;

                if branch == "HEAD" {
                    return Err(Error::NotBranch);
                }

                branches.insert(root.clone(), branch.clone());

                // Get the final `allow_branch` value
                let allow_branch_default_value = String::from("master");
                let allow_branch = self.allow_branch.as_ref().unwrap_or_else(|| {
                    config
                        .allow_branch
                        .as_ref()
                        .unwrap_or(&allow_branch_default_value)
                });

                // Treat `main` as `master`
                let test_branch = if branch == "main" && allow_branch.as_str() == "master" {
                    "master".into()
                } else {
                    branch.clone()
                };

                let pattern = Glob::new(allow_branch)?;

                if !pattern.compile_matcher().is_match(test_branch) {
                    return Err(Error::BranchNotAllowed {
                        branch,
                        pattern: pattern.glob().to_string(),
                    });
                }

                if !self.no_git_push {
                    let remote_branch = format!("{}/{}", self.git_remote, branch);

                    let (_, out, _) = git(
                        root,
                        &[
                            "show-ref",
                            "--verify",
                            &format!("refs/remotes/{}", remote_branch),
                        ],
                    )?;

                    if out.is_empty() {
                        return Err(Error::NoRemote {
                            remote: self.git_remote.clone(),
                            branch,
                        });
                    }

                    git(root, &["remote", "update"])?;

                    let (_, out, _) = git(
                        root,
                        &[
                            "rev-list",
                            "--left-only",
                            "--count",
                            &format!("{}...{}", remote_branch, branch),
                        ],
                    )?;

                    if out != "0" {
                        return Err(Error::BehindRemote {
                            branch,
                            upstream: remote_branch,
                        });
                    }
                }
            }
        }

        Ok(branches)
    }

    pub fn commit(
        &self,
        roots: &[Utf8PathBuf],
        workspace_root: &Utf8PathBuf,
        new_version: &Option<Version>,
        new_versions: &Map<Utf8PathBuf, Map<String, RepoVersion>>,
        branches: &Map<Utf8PathBuf, String>,
        config: &WorkspaceConfig,
    ) -> Result<(), Error> {
        if !self.no_git_commit {
            for root in roots {
                info!("version", format!("committing changes in {}", root));

                let branch = branches.get(root).expect(INTERNAL_ERR);
                let added = git(root, &["add", "-u"])?;

                if !added.0.success() {
                    return Err(Error::NotAdded(added.1, added.2));
                }

                let (staged_status, staged_out, _) =
                    git(root, &["diff", "--cached", "--name-only"])?;

                if !staged_status.success() {
                    return Err(Error::Bail);
                }

                if staged_out.is_empty() {
                    continue;
                }

                let repo_versions = new_versions.get(root).cloned().unwrap_or_default();
                let commit_versions = repo_versions
                    .iter()
                    .map(|(name, data)| (name.clone(), data.version.clone()))
                    .collect();

                let mut args = vec!["commit".to_string()];

                if self.amend {
                    args.push("--amend".to_string());
                    args.push("--no-edit".to_string());
                } else {
                    args.push("-m".to_string());

                    let mut msg = "Release %v";

                    if let Some(supplied) = &self.message {
                        msg = supplied;
                    }

                    let mut msg = self.commit_msg(msg, &commit_versions);

                    let version_label = if let Some(version) = new_version {
                        version.to_string()
                    } else if commit_versions.len() == 1 {
                        commit_versions
                            .iter()
                            .next()
                            .map(|(name, version)| format!("{}@{}", name, version))
                            .unwrap_or_else(|| "independent packages".to_string())
                    } else {
                        "independent packages".to_string()
                    };

                    msg = msg.replace("%v", &version_label);

                    args.push(msg);
                }

                let committed = git(root, &args.iter().map(|x| x.as_str()).collect::<Vec<_>>())?;

                if !committed.0.success() {
                    return Err(Error::NotCommitted(committed.1, committed.2));
                }

                if !self.no_git_tag {
                    info!("version", format!("tagging in {}", root));
                    let has_global_tag =
                        !self.no_global_tag && root == workspace_root && new_version.is_some();

                    if has_global_tag {
                        if let Some(version) = new_version {
                            let tag = format!("{}{}", &self.tag_prefix, version);
                            self.tag(root, &tag, &tag)?;
                        }
                    }

                    if !(self.no_individual_tags || config.no_individual_tags.unwrap_or_default()) {
                        for (p, data) in &repo_versions {
                            if self.should_skip_individual_tag(workspace_root, root, data, has_global_tag)
                            {
                                continue;
                            }

                            let tag =
                                self.individual_tag(workspace_root, root, p, &data.version, data.independent);
                            self.tag(root, &tag, &tag)?;
                        }
                    }
                }

                if !self.no_git_push {
                    info!("git", format!("pushing from {}", root));

                    let pushed = git(root, &["push", "--follow-tags", &self.git_remote, branch])?;

                    if !pushed.0.success() {
                        return Err(Error::NotPushed(pushed.1, pushed.2));
                    }
                }
            }
        }

        Ok(())
    }

    fn tag(&self, root: &Utf8PathBuf, tag: &str, msg: &str) -> Result<(), Error> {
        let tagged = git(root, &["tag", tag, "-m", msg])?;

        if !tagged.0.success() {
            return Err(Error::NotTagged(tag.to_string(), tagged.1, tagged.2));
        }

        Ok(())
    }

    fn commit_msg(&self, msg: &str, new_versions: &Map<String, Version>) -> String {
        format!(
            "{}\n\n{}\n\nGenerated by cargo-workspaces",
            msg,
            new_versions
                .iter()
                .map(|x| format!("{}@{}", x.0, x.1))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }

    fn individual_tag(
        &self,
        workspace_root: &Utf8PathBuf,
        root: &Utf8PathBuf,
        pkg_name: &str,
        version: &Version,
        independent: bool,
    ) -> String {
        let prefix = if independent && root != workspace_root {
            &self.tag_prefix
        } else {
            &self.individual_tag_prefix.replace("%n", pkg_name)
        };

        format!("{}{}", prefix, version)
    }

    fn should_skip_individual_tag(
        &self,
        workspace_root: &Utf8PathBuf,
        root: &Utf8PathBuf,
        version: &RepoVersion,
        has_global_tag: bool,
    ) -> bool {
        has_global_tag && root == workspace_root && version.root
    }
}

#[cfg(test)]
mod tests {
    use super::{GitOpt, RepoVersion};

    use camino::Utf8PathBuf;
    use semver::Version;

    fn git_opt() -> GitOpt {
        GitOpt {
            no_git_commit: false,
            allow_branch: None,
            amend: false,
            message: None,
            no_git_tag: false,
            no_individual_tags: false,
            no_global_tag: false,
            tag_prefix: "v".to_string(),
            individual_tag_prefix: "%n@".to_string(),
            no_git_push: false,
            git_remote: "origin".to_string(),
        }
    }

    #[test]
    fn individual_tag_uses_global_prefix_for_independent_package_in_separate_repo() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");
        let root = Utf8PathBuf::from("/workspace/submodule");
        let version = Version::parse("1.2.3").expect("valid version");

        let tag = git.individual_tag(&workspace_root, &root, "crate", &version, true);

        assert_eq!(tag, "v1.2.3");
    }

    #[test]
    fn root_package_tag_is_skipped_when_global_tag_exists() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");

        let skipped = git.should_skip_individual_tag(
            &workspace_root,
            &workspace_root,
            &RepoVersion {
                version: Version::parse("1.2.3").expect("valid version"),
                independent: false,
                root: true,
            },
            true,
        );

        assert!(skipped);
    }

    #[test]
    fn non_root_package_tag_is_not_skipped_when_global_tag_exists() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");

        let skipped = git.should_skip_individual_tag(
            &workspace_root,
            &workspace_root,
            &RepoVersion {
                version: Version::parse("1.2.3").expect("valid version"),
                independent: false,
                root: false,
            },
            true,
        );

        assert!(!skipped);
    }

    #[test]
    fn root_package_tag_is_not_skipped_without_global_tag() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");

        let skipped = git.should_skip_individual_tag(
            &workspace_root,
            &workspace_root,
            &RepoVersion {
                version: Version::parse("1.2.3").expect("valid version"),
                independent: false,
                root: true,
            },
            false,
        );

        assert!(!skipped);
    }

    #[test]
    fn individual_tag_uses_package_prefix_for_independent_package_in_workspace_repo() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");
        let version = Version::parse("1.2.3").expect("valid version");

        let tag = git.individual_tag(&workspace_root, &workspace_root, "crate", &version, true);

        assert_eq!(tag, "crate@1.2.3");
    }

    #[test]
    fn individual_tag_uses_package_prefix_for_non_independent_package_in_separate_repo() {
        let git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");
        let root = Utf8PathBuf::from("/workspace/submodule");
        let version = Version::parse("1.2.3").expect("valid version");

        let tag = git.individual_tag(&workspace_root, &root, "crate", &version, false);

        assert_eq!(tag, "crate@1.2.3");
    }

    #[test]
    fn individual_tag_uses_custom_global_prefix_for_independent_package_in_separate_repo() {
        let mut git = git_opt();
        let workspace_root = Utf8PathBuf::from("/workspace");
        let root = Utf8PathBuf::from("/workspace/submodule");
        let version = Version::parse("1.2.3").expect("valid version");

        git.tag_prefix = "release-".to_string();

        let tag = git.individual_tag(&workspace_root, &root, "crate", &version, true);

        assert_eq!(tag, "release-1.2.3");
    }
}
