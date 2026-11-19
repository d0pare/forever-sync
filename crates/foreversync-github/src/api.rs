//! The REST calls setup and the privacy guard need.

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::config::{API_BASE, DEFAULT_DATA_REPO};
use crate::{GithubError, Result, agent};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RepoInfo {
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub clone_url: String,
    pub size: u64,
    #[serde(default)]
    pub default_branch: Option<String>,
    pub owner: Owner,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Owner {
    pub login: String,
}

/// The repo Forever Sync stores data in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRepo {
    pub repo: RepoInfo,
    /// False when the repo is empty or lacks `foreversync.json` (first machine).
    pub initialized: bool,
}

pub struct Client {
    agent: ureq::Agent,
    token: String,
}

#[derive(Deserialize)]
struct Installations {
    installations: Vec<Installation>,
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct Repos {
    repositories: Vec<RepoInfo>,
}

#[derive(Deserialize)]
pub struct User {
    pub login: String,
}

impl Client {
    pub fn new(token: impl Into<String>) -> Self {
        Self { agent: agent(), token: token.into() }
    }

    fn get(&self, path: &str) -> Result<(u16, String)> {
        let mut resp = self
            .agent
            .get(format!("{API_BASE}{path}"))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()?;
        let status = resp.status().as_u16();
        Ok((status, resp.body_mut().read_to_string()?))
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let (status, body) = self.get(path)?;
        if status != 200 {
            return Err(GithubError::Status { status, what: path.into(), body });
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub fn user(&self) -> Result<User> {
        self.get_json("/user")
    }

    pub fn repo(&self, full_name: &str) -> Result<RepoInfo> {
        self.get_json(&format!("/repos/{full_name}"))
    }

    /// Every repo the app is installed on that the user can access.
    pub fn accessible_repos(&self) -> Result<Vec<RepoInfo>> {
        let inst: Installations = self.get_json("/user/installations?per_page=100")?;
        let mut repos = Vec::new();
        for i in inst.installations {
            let page: Repos = self.get_json(&format!("/user/installations/{}/repositories?per_page=100", i.id))?;
            repos.extend(page.repositories);
        }
        Ok(repos)
    }

    /// Does `path` exist on the default branch? An empty repo has no files.
    pub fn has_file(&self, full_name: &str, path: &str) -> Result<bool> {
        let (status, body) = self.get(&format!("/repos/{full_name}/contents/{path}"))?;
        match status {
            200 => Ok(true),
            404 | 409 => Ok(false), // 409: repository is empty
            _ => Err(GithubError::Status { status, what: path.into(), body }),
        }
    }

    /// Finds the data repo among accessible repos: one with `foreversync.json`
    /// wins; otherwise a repo named `forever-sync-data` (to initialize).
    pub fn find_data_repo(&self) -> Result<Option<DataRepo>> {
        let repos = self.accessible_repos()?;
        for r in &repos {
            if self.has_file(&r.full_name, "foreversync.json")? {
                return Ok(Some(DataRepo { repo: r.clone(), initialized: true }));
            }
        }
        Ok(repos
            .into_iter()
            .find(|r| r.name == DEFAULT_DATA_REPO)
            .map(|repo| DataRepo { repo, initialized: false }))
    }
}
