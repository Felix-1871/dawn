// SPDX-License-Identifier: GPL-3.0-or-later

//! Which package repositories an online install needs, read from the
//! `pacman.conf` pacstrap uses. Both the `check_online` request and
//! step 1's reachability check (SPEC.md "Install pipeline") use this.

/// One repository section and its mirrors, in the order pacman tries
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub servers: Vec<String>,
}

impl Repo {
    /// The database file an online install downloads first; if a mirror
    /// serves this, it's reachable.
    pub fn database_urls(&self) -> Vec<String> {
        self.servers
            .iter()
            .map(|server| format!("{}/{}.db", server.trim_end_matches('/'), self.name))
            .collect()
    }
}

/// Parses `pacman.conf`: every section except `[options]` is a
/// repository, with `Server =` lines directly or through `Include =`
/// files (usually a mirrorlist). `read_include` returns an included
/// file's contents; one that can't be read contributes no servers.
/// `$repo` and `$arch` are expanded the way pacman does.
pub fn parse_pacman_conf(conf: &str, read_include: &dyn Fn(&str) -> Option<String>) -> Vec<Repo> {
    let mut repos: Vec<Repo> = Vec::new();
    for line in conf.lines() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if name != "options" {
                repos.push(Repo {
                    name: name.to_string(),
                    servers: Vec::new(),
                });
            }
            continue;
        }
        let Some(repo) = repos.last_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Server" => repo.servers.push(expand(value.trim(), &repo.name)),
            "Include" => {
                if let Some(included) = read_include(value.trim()) {
                    let name = repo.name.clone();
                    for inner in included.lines() {
                        let inner = strip_comment(inner);
                        if let Some((key, value)) = inner.split_once('=')
                            && key.trim() == "Server"
                        {
                            repo.servers.push(expand(value.trim(), &name));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    repos
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("").trim()
}

fn expand(server: &str, repo: &str) -> String {
    server
        .replace("$repo", repo)
        .replace("$arch", std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = "\
[options]
Architecture = auto
Include = /etc/should-not-be-read

# A comment
[core]
Include = /etc/pacman.d/mirrorlist

[luminos-repository]
SigLevel = Required
Server = https://example.invalid/$repo/$arch  # trailing comment
";

    const MIRRORLIST: &str = "\
## Germany
#Server = https://commented.invalid/$repo/os/$arch
Server = https://mirror.invalid/$repo/os/$arch
";

    fn read(path: &str) -> Option<String> {
        (path == "/etc/pacman.d/mirrorlist").then(|| MIRRORLIST.to_string())
    }

    #[test]
    fn reads_servers_directly_and_through_includes() {
        let repos = parse_pacman_conf(CONF, &read);
        let arch = std::env::consts::ARCH;
        assert_eq!(
            repos,
            vec![
                Repo {
                    name: "core".into(),
                    servers: vec![format!("https://mirror.invalid/core/os/{arch}")],
                },
                Repo {
                    name: "luminos-repository".into(),
                    servers: vec![format!("https://example.invalid/luminos-repository/{arch}")],
                },
            ]
        );
    }

    #[test]
    fn database_urls_point_at_the_repo_database() {
        let repo = Repo {
            name: "dawnlocal".into(),
            servers: vec!["http://10.0.2.2:8080/".into()],
        };
        assert_eq!(repo.database_urls(), ["http://10.0.2.2:8080/dawnlocal.db"]);
    }

    #[test]
    fn an_unreadable_include_adds_no_servers() {
        let repos = parse_pacman_conf("[core]\nInclude = /missing\n", &|_| None);
        assert_eq!(repos[0].servers, Vec::<String>::new());
    }
}
