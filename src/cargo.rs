//! What cargo says about the workspaces a bump touches, read from `cargo
//! metadata` rather than parsed out of the manifests by hand.

use std::path::Path;

use serde::Deserialize;

use crate::Result;
use crate::config::Config;
use crate::sh::Sh;

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: String,
    /// `[]` is how cargo reports `publish = false`; null is "anywhere".
    publish: Option<Vec<String>>,
}

/// One Cargo workspace a bump touches: the manifest carrying the version,
/// every manifest in it, and every member's name.
///
/// Every manifest, because an internal `{ path, version }` pin is only
/// usually declared in the root's `[workspace.dependencies]`. Every member's
/// name, because a `{ path, version }` pin is not necessarily internal: a pin
/// is moved when it names a member, and left alone otherwise.
pub struct Workspace {
    pub manifest: String,
    pub manifests: Vec<String>,
    pub members: Vec<String>,
}

fn packages(sh: &Sh, dir: Option<&str>) -> Result<Vec<Package>> {
    let json = sh.capture_in(
        dir,
        "cargo",
        &["metadata", "--no-deps", "--format-version", "1"],
    )?;
    let metadata: Metadata =
        fig_json::from_str(&json).map_err(|e| format!("could not read `cargo metadata`: {e}"))?;
    Ok(metadata.packages)
}

fn parent(manifest: &str) -> Option<String> {
    Path::new(manifest)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|d| !d.is_empty())
}

pub fn workspaces(sh: &Sh, config: &Config) -> Result<Vec<Workspace>> {
    let mut list: Vec<(String, Option<String>)> = Vec::new();
    if config.cargo_workspace {
        list.push((config.manifest.clone(), None));
    }
    for mirror in &config.mirrors {
        list.push((mirror.clone(), parent(mirror)));
    }
    let root = sh.root.canonicalize().unwrap_or_else(|_| sh.root.clone());
    list.into_iter()
        .map(|(manifest, dir)| {
            let packages = packages(sh, dir.as_deref())?;
            let manifests = packages
                .iter()
                .map(|p| relative(&root, &p.manifest_path))
                .collect();
            let members = packages.into_iter().map(|p| p.name).collect();
            Ok(Workspace {
                manifest,
                manifests,
                members,
            })
        })
        .collect()
}

fn relative(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Every crate a release would upload, across the canonical workspace and
/// every mirror — twig's crates all live in a mirror.
pub fn publishable(sh: &Sh, config: &Config) -> Result<Vec<String>> {
    if !config.publishes {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<Option<String>> = Vec::new();
    if config.cargo_workspace {
        dirs.push(None);
    }
    dirs.extend(config.mirrors.iter().map(|m| parent(m)));
    let mut names: Vec<String> = Vec::new();
    for dir in dirs {
        for p in packages(sh, dir.as_deref())? {
            let published = p.publish.as_ref().is_none_or(|targets| !targets.is_empty());
            if published && !names.contains(&p.name) {
                names.push(p.name);
            }
        }
    }
    Ok(names)
}
