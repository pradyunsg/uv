use crate::commit_info::CacheCommit;
use crate::timestamp::Timestamp;

use serde::Deserialize;
use std::cmp::max;
use std::io;
use std::path::{Path, PathBuf};
use tracing::debug;

/// The information used to determine whether a built distribution is up-to-date, based on the
/// timestamps of relevant files, the current commit of a repository, etc.
#[derive(Default, Debug, Clone, Hash, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(try_from = "CacheInfoWire")]
pub struct CacheInfo {
    /// The timestamp of the most recent `ctime` of any relevant files, at the time of the build.
    /// The timestamp will typically be the maximum of the `ctime` values of the `pyproject.toml`,
    /// `setup.py`, and `setup.cfg` files, if they exist; however, users can provide additional
    /// files to timestamp via the `cache-keys` field.
    timestamp: Option<Timestamp>,
    /// The commit at which the distribution was built.
    commit: Option<CacheCommit>,
}

impl CacheInfo {
    /// Return the [`CacheInfo`] for a given timestamp.
    pub fn from_timestamp(timestamp: Timestamp) -> Self {
        Self {
            timestamp: Some(timestamp),
            ..Self::default()
        }
    }

    /// Compute the cache info for a given path, which may be a file or a directory.
    pub fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = fs_err::metadata(path)?;
        if metadata.is_file() {
            Self::from_file(path)
        } else {
            Self::from_directory(path)
        }
    }

    /// Compute the cache info for a given directory.
    pub fn from_directory(directory: &Path) -> io::Result<Self> {
        let mut commit = None;
        let mut timestamp = None;

        // Read the cache keys.
        let cache_keys =
            if let Ok(contents) = fs_err::read_to_string(directory.join("pyproject.toml")) {
                if let Ok(pyproject_toml) = toml::from_str::<PyProjectToml>(&contents) {
                    pyproject_toml
                        .tool
                        .and_then(|tool| tool.uv)
                        .and_then(|tool_uv| tool_uv.cache_keys)
                } else {
                    None
                }
            } else {
                None
            };

        // If no cache keys were defined, use the defaults.
        let cache_keys = cache_keys.unwrap_or_else(|| {
            vec![
                CacheKey::Path(directory.join("pyproject.toml")),
                CacheKey::Path(directory.join("setup.py")),
                CacheKey::Path(directory.join("setup.cfg")),
            ]
        });

        // Incorporate any additional timestamps or VCS information.
        for cache_key in &cache_keys {
            match cache_key {
                CacheKey::Path(file) | CacheKey::File { file } => {
                    timestamp = max(
                        timestamp,
                        file.metadata()
                            .ok()
                            .filter(std::fs::Metadata::is_file)
                            .as_ref()
                            .map(Timestamp::from_metadata),
                    );
                }
                CacheKey::Git { git: true } => match CacheCommit::from_repository(directory) {
                    Ok(commit_info) => commit = Some(commit_info),
                    Err(err) => {
                        debug!("Failed to read the current commit: {err}");
                    }
                },
                CacheKey::Git { git: false } => {}
            }
        }

        Ok(Self { timestamp, commit })
    }

    /// Compute the cache info for a given file, assumed to be a binary or source distribution
    /// represented as (e.g.) a `.whl` or `.tar.gz` archive.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, io::Error> {
        let metadata = fs_err::metadata(path.as_ref())?;
        let timestamp = Timestamp::from_metadata(&metadata);
        Ok(Self {
            timestamp: Some(timestamp),
            ..Self::default()
        })
    }

    pub fn is_empty(&self) -> bool {
        self.timestamp.is_none() && self.commit.is_none()
    }
}

#[derive(Debug, serde::Deserialize)]
struct TimestampCommit {
    timestamp: Option<Timestamp>,
    commit: Option<CacheCommit>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum CacheInfoWire {
    /// For backwards-compatibility, enable deserializing [`CacheInfo`] structs that are solely
    /// represented by a timestamp.
    Timestamp(Timestamp),
    /// A [`CacheInfo`] struct that includes both a timestamp and a commit.
    TimestampCommit(TimestampCommit),
}

impl From<CacheInfoWire> for CacheInfo {
    fn from(wire: CacheInfoWire) -> Self {
        match wire {
            CacheInfoWire::Timestamp(timestamp) => Self {
                timestamp: Some(timestamp),
                ..Self::default()
            },
            CacheInfoWire::TimestampCommit(TimestampCommit { timestamp, commit }) => {
                Self { timestamp, commit }
            }
        }
    }
}

/// A `pyproject.toml` with an (optional) `[tool.uv]` section.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PyProjectToml {
    tool: Option<Tool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Tool {
    uv: Option<ToolUv>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ToolUv {
    cache_keys: Option<Vec<CacheKey>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(untagged, rename_all = "kebab-case", deny_unknown_fields)]
pub enum CacheKey {
    /// Ex) `"Cargo.lock"`
    Path(PathBuf),
    /// Ex) `{ file = "Cargo.lock" }`
    File { file: PathBuf },
    /// Ex) `{ git = true }`
    Git { git: bool },
}