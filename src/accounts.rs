//! Commit identities and GitHub accounts.
//!
//! A [`Profile`] pairs a commit name and email with the name of a GitHub
//! account that Git Credential Manager already knows. Nothing secret is stored
//! here: logins stay in the credential manager, and the profile only tells Git
//! which of them to use.
//!
//! The chosen profile reaches Git as one-off `-c` options on the commands that
//! create commits or talk to a remote, so a repository's own config is never
//! rewritten. [`set_for`] publishes the profile for a worktree and
//! [`config_args`] hands the options to the command runner.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Unique name shown in menus and used as the key in settings.
    pub label: String,
    /// Commit author and committer name.
    pub name: String,
    pub email: String,
    /// GitHub account whose stored login is used for push, fetch and pull.
    pub github: Option<String>,
}

/// The first problem found in a profile being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileIssue {
    Label,
    Name,
    Email,
    Github,
}

impl Profile {
    pub fn validate(&self) -> Result<(), ProfileIssue> {
        if !clean(&self.label) {
            return Err(ProfileIssue::Label);
        }
        if !clean(&self.name) || self.name.contains(['<', '>']) {
            return Err(ProfileIssue::Name);
        }
        let email_ok = clean(&self.email)
            && !self.email.contains(['<', '>', ' '])
            && self
                .email
                .split_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
        if !email_ok {
            return Err(ProfileIssue::Email);
        }
        if self.github.as_deref().is_some_and(|login| !valid_github(login)) {
            return Err(ProfileIssue::Github);
        }
        Ok(())
    }
}

/// Non-empty after trimming and free of control characters, so a value can
/// never smuggle a line break into a log, a settings file or a Git config.
fn clean(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

/// GitHub login rules: letters, digits and hyphens, at most 39 characters, and
/// no hyphen at either end.
pub fn valid_github(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Account that owns a GitHub remote, lowercased: `https://github.com/o/r.git`,
/// `git@github.com:o/r.git` and `ssh://git@github.com/o/r` all give `o`.
pub fn github_owner(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('@').map_or(rest, |(_, host)| host),
        None => url.split_once('@').map_or(url, |(_, host)| host),
    };
    let (host, path) = rest.split_once(['/', ':'])?;
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let owner = path.trim_start_matches('/').split('/').next()?;
    valid_github(owner).then(|| owner.to_ascii_lowercase())
}

/// Why a profile applies to a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Picked for this repository.
    Chosen,
    /// A remote belongs to the profile's GitHub account.
    Remote,
    /// The default profile.
    Default,
}

pub struct Resolved<'a> {
    pub profile: &'a Profile,
    pub source: Source,
    /// Owners of the repository's GitHub remotes when none of them is the
    /// profile's account. Only reported for the default profile: a profile
    /// the user picked is never second-guessed.
    pub other_owners: Option<String>,
}

/// Profile for one repository. In order: the one chosen for it (an empty
/// choice means "use Git's own config"), the first profile whose GitHub
/// account owns a remote, then the default.
pub fn resolve<'a>(
    profiles: &'a [Profile],
    default: Option<&str>,
    chosen: Option<&str>,
    remote_urls: &[String],
) -> Option<Resolved<'a>> {
    let find = |label: &str| profiles.iter().find(|profile| profile.label == label);
    let found = |profile, source| Resolved { profile, source, other_owners: None };
    match chosen {
        Some("") => return None,
        Some(label) => {
            if let Some(profile) = find(label) {
                return Some(found(profile, Source::Chosen));
            }
        }
        None => {}
    }
    let owners: Vec<String> = remote_urls.iter().filter_map(|url| github_owner(url)).collect();
    let owned = profiles.iter().find(|profile| {
        profile
            .github
            .as_deref()
            .is_some_and(|login| owners.iter().any(|owner| owner.eq_ignore_ascii_case(login)))
    });
    if let Some(profile) = owned {
        return Some(found(profile, Source::Remote));
    }
    let profile = default.and_then(find)?;
    let other_owners = (profile.github.is_some() && !owners.is_empty()).then(|| {
        let mut owners = owners.clone();
        owners.dedup();
        owners.join(", ")
    });
    Some(Resolved { profile, source: Source::Default, other_owners })
}

/// Git commands that create commits (author identity) or reach a remote
/// (credentials). Everything else runs untouched.
const IDENTITY_COMMANDS: &[&str] = &[
    "commit", "cherry-pick", "revert", "merge", "rebase", "pull", "stash", "tag", "push", "fetch",
    "remote",
];

/// The Git subcommand of an argument list that starts with `-C <dir>` and any
/// number of `-c <key=value>` pairs.
pub fn subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut rest = args.iter();
    while let Some(&arg) = rest.next() {
        if arg == "-C" || arg == "-c" {
            rest.next();
        } else {
            return Some(arg);
        }
    }
    None
}

#[derive(Clone)]
struct Entry {
    name: String,
    email: String,
    github: Option<String>,
}

fn registry() -> &'static RwLock<HashMap<String, Entry>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Publish (or clear) the profile used for commands run in `worktree`.
pub fn set_for(worktree: &str, profile: Option<&Profile>) {
    let mut map = registry().write().unwrap();
    match profile {
        Some(profile) => {
            map.insert(
                worktree.to_string(),
                Entry {
                    name: profile.name.clone(),
                    email: profile.email.clone(),
                    github: profile.github.clone(),
                },
            );
        }
        None => {
            map.remove(worktree);
        }
    }
}

/// `-c` options to put after `-C <worktree>` for a command, empty when the
/// worktree has no profile or the command needs neither identity nor login.
/// Both are plain config values, so there is no shell to escape for.
pub fn config_args(worktree: &str, args: &[&str]) -> Vec<String> {
    if !subcommand(args).is_some_and(|sub| IDENTITY_COMMANDS.contains(&sub)) {
        return Vec::new();
    }
    let Some(entry) = registry().read().unwrap().get(worktree).cloned() else {
        return Vec::new();
    };
    let mut config = vec![format!("user.name={}", entry.name), format!("user.email={}", entry.email)];
    if let Some(login) = entry.github {
        config.push(format!("credential.https://github.com.username={login}"));
    }
    config.into_iter().flat_map(|value| ["-c".to_string(), value]).collect()
}

/// Git's own `user.name` and `user.email` (global and system config), to
/// prefill a new profile. `None` for a value that is unset or when Git is
/// missing.
pub fn git_identity() -> (Option<String>, Option<String>) {
    let Some(git) = crate::git::windows_git() else {
        return (None, None);
    };
    let get = |key: &str| {
        crate::process::Command::new(git)
            .args(["config", "--get", key])
            .output()
            .ok()
            .filter(|out| out.success())
            .map(|out| out.stdout_text().trim().to_string())
            .filter(|value| !value.is_empty())
    };
    (get("user.name"), get("user.email"))
}

/// GitHub accounts Git Credential Manager has a login for. Empty when it is
/// not installed or has none; this only reads account names.
pub fn stored_github_accounts() -> Vec<String> {
    let Some(git) = crate::git::windows_git() else {
        return Vec::new();
    };
    let Ok(out) = crate::process::Command::new(git)
        .args(["credential-manager", "github", "list"])
        .output()
    else {
        return Vec::new();
    };
    if !out.success() {
        return Vec::new();
    }
    out.stdout_text()
        .lines()
        .map(str::trim)
        .filter(|login| valid_github(login))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(label: &str, github: Option<&str>) -> Profile {
        Profile {
            label: label.into(),
            name: "Ada Lovelace".into(),
            email: format!("{label}@example.com"),
            github: github.map(str::to_string),
        }
    }

    #[test]
    fn github_owner_reads_every_remote_spelling() {
        for url in [
            "https://github.com/Press-Again/SpurGit.git",
            "https://user@github.com/press-again/SpurGit",
            "git@github.com:press-again/SpurGit.git",
            "ssh://git@github.com/press-again/SpurGit.git",
            "git://github.com/press-again/SpurGit.git",
        ] {
            assert_eq!(github_owner(url).as_deref(), Some("press-again"), "{url}");
        }
        assert_eq!(github_owner("https://gitlab.com/press-again/x.git"), None);
        assert_eq!(github_owner("https://github.com/"), None);
        assert_eq!(github_owner("../remote.git"), None);
        assert_eq!(github_owner("https://notgithub.com/o/r"), None);
    }

    #[test]
    fn validation_names_the_first_problem() {
        let ok = profile("work", Some("press-again"));
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(Profile { label: " ".into(), ..ok.clone() }.validate(), Err(ProfileIssue::Label));
        assert_eq!(Profile { name: "A\nB".into(), ..ok.clone() }.validate(), Err(ProfileIssue::Name));
        assert_eq!(Profile { name: "A <b>".into(), ..ok.clone() }.validate(), Err(ProfileIssue::Name));
        for email in ["", "no-at", "a@b", "a b@c.d", "@c.d", "a@c.d\n"] {
            assert_eq!(
                Profile { email: email.into(), ..ok.clone() }.validate(),
                Err(ProfileIssue::Email),
                "{email:?}"
            );
        }
        for login in ["-a", "a-", "a_b", "a b", &"x".repeat(40)] {
            assert_eq!(
                Profile { github: Some(login.into()), ..ok.clone() }.validate(),
                Err(ProfileIssue::Github),
                "{login:?}"
            );
        }
        assert_eq!(Profile { github: None, ..ok }.validate(), Ok(()));
    }

    #[test]
    fn a_profile_is_picked_by_choice_then_remote_then_default() {
        let profiles = [profile("home", None), profile("work", Some("Press-Again")), profile("oss", Some("spur"))];
        let remotes = vec!["https://github.com/press-again/SpurGit.git".to_string()];

        let by_choice = resolve(&profiles, Some("home"), Some("oss"), &remotes).unwrap();
        assert_eq!((by_choice.profile.label.as_str(), by_choice.source), ("oss", Source::Chosen));
        assert!(by_choice.other_owners.is_none(), "a chosen profile is never second-guessed");

        let by_remote = resolve(&profiles, Some("home"), None, &remotes).unwrap();
        assert_eq!((by_remote.profile.label.as_str(), by_remote.source), ("work", Source::Remote));

        let by_default = resolve(&profiles, Some("home"), None, &[]).unwrap();
        assert_eq!((by_default.profile.label.as_str(), by_default.source), ("home", Source::Default));

        // A choice for a deleted profile falls back to the automatic rules.
        let stale = resolve(&profiles, None, Some("gone"), &remotes).unwrap();
        assert_eq!(stale.profile.label, "work");

        // An empty choice means "use Git's own config".
        assert!(resolve(&profiles, Some("home"), Some(""), &remotes).is_none());
        assert!(resolve(&profiles, None, None, &[]).is_none());
    }

    #[test]
    fn the_default_profile_warns_when_the_remote_belongs_to_someone_else() {
        let profiles = [profile("home", Some("me"))];
        let theirs = vec!["https://github.com/Acme/tool.git".to_string(), "git@github.com:acme/tool.git".to_string()];
        let resolved = resolve(&profiles, Some("home"), None, &theirs).unwrap();
        assert_eq!(resolved.other_owners.as_deref(), Some("acme"));

        let none = resolve(&profiles, Some("home"), None, &["https://gitlab.com/x/y".to_string()]).unwrap();
        assert!(none.other_owners.is_none(), "only GitHub remotes are compared");
        let no_account = [profile("local", None)];
        assert!(resolve(&no_account, Some("local"), None, &theirs).unwrap().other_owners.is_none());
    }

    #[test]
    fn subcommand_skips_leading_options() {
        assert_eq!(subcommand(&["-C", "/w", "push", "--progress"]), Some("push"));
        assert_eq!(subcommand(&["-C", "/w", "-c", "a=b", "commit", "--file=-"]), Some("commit"));
        assert_eq!(subcommand(&["-C", "/w"]), None);
        assert_eq!(subcommand(&[]), None);
    }

    #[test]
    fn config_args_apply_only_to_commands_that_need_an_identity() {
        let wt = "/mnt/c/accounts-test/repo";
        let p = Profile { name: "Ada".into(), ..profile("work", Some("press-again")) };
        set_for(wt, Some(&p));

        let push = config_args(wt, &["-C", wt, "push", "--progress", "origin", "main"]);
        assert_eq!(
            push,
            [
                "-c", "user.name=Ada", "-c", "user.email=work@example.com", "-c",
                "credential.https://github.com.username=press-again"
            ]
        );
        assert!(!config_args(wt, &["-C", wt, "commit", "--file=-"]).is_empty());
        for read_only in ["status", "log", "diff", "rev-parse", "config"] {
            assert!(config_args(wt, &["-C", wt, read_only]).is_empty(), "{read_only}");
        }
        assert!(config_args("/mnt/c/other", &["-C", "/mnt/c/other", "commit"]).is_empty());

        let local = Profile { github: None, ..p };
        set_for(wt, Some(&local));
        assert_eq!(config_args(wt, &["-C", wt, "commit"]).len(), 4, "no login option without an account");
        set_for(wt, None);
        assert!(config_args(wt, &["-C", wt, "commit"]).is_empty());
    }
}
