use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(error) = run() {
        eprintln!("repo-check: {error}");
        std::process::exit(1);
    }
    println!("repo-check: repository contract passed");
}

fn run() -> Result<()> {
    let root = workspace_root()?;
    required_paths(&root)?;
    dependency_layers(&root)?;
    unsafe_boundary(&root)?;
    markdown_links(&root)?;
    workflow_pins(&root)?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|error| format!("running cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing cargo metadata: {error}"))?;
    metadata
        .get("workspace_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata did not report workspace_root".to_owned())
}

fn required_paths(root: &Path) -> Result<()> {
    let required = [
        "AGENTS.md",
        "ARCHITECTURE.md",
        ".codex/hooks.json",
        "justfile",
        "rust-toolchain.toml",
        "docs/index.md",
        "docs/testing.md",
        "docs/security.md",
        "docs/github.md",
        "docs/quality.md",
        "docs/plans/active/index.md",
        "docs/plans/completed",
        "docs/tech-debt.md",
        ".github/pull_request_template.md",
        ".github/dependabot.yml",
        ".github/agent-policy.yml",
        ".github/actionlint.yaml",
        "tests/viewer-smoke.sh",
        "tests/viewer-smoke.js",
    ];
    let missing = required
        .iter()
        .filter(|path| !root.join(path).exists())
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "missing required repository paths: {}",
            missing.join(", ")
        ))
    }
}

fn dependency_layers(root: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("running cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err("cargo metadata failed while checking dependency layers".to_owned());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing package metadata: {error}"))?;
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata has no packages".to_owned())?;
    let workspace_members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or_else(|| "cargo metadata has no workspace members".to_owned())?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();

    let mut names = BTreeMap::new();
    for package in packages {
        let id = package
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "package metadata has no id".to_owned())?;
        if !workspace_members.contains(id) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "package metadata has no name".to_owned())?;
        let dependencies = package
            .get("dependencies")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("package {name} has no dependencies"))?
            .iter()
            .filter_map(|dependency| dependency.get("name").and_then(Value::as_str))
            .filter(|dependency| {
                [
                    "slice-core",
                    "slice-capture",
                    "slice-collector",
                    "slice-render",
                    "slice-ebpf",
                    "slice-cli",
                    "repo-check",
                ]
                .contains(dependency)
            })
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        names.insert(name.to_owned(), dependencies);
    }

    let expected = BTreeMap::from([
        ("slice-core", BTreeSet::new()),
        ("slice-capture", BTreeSet::from(["slice-core".to_owned()])),
        ("slice-collector", BTreeSet::from(["slice-core".to_owned()])),
        ("slice-render", BTreeSet::from(["slice-core".to_owned()])),
        (
            "slice-ebpf",
            BTreeSet::from([
                "slice-capture".to_owned(),
                "slice-core".to_owned(),
                "slice-collector".to_owned(),
            ]),
        ),
        (
            "slice-cli",
            BTreeSet::from([
                "slice-core".to_owned(),
                "slice-capture".to_owned(),
                "slice-ebpf".to_owned(),
                "slice-render".to_owned(),
            ]),
        ),
        ("repo-check", BTreeSet::new()),
    ]);
    for (package, allowed) in expected {
        let actual = names
            .get(package)
            .ok_or_else(|| format!("expected workspace package {package}"))?;
        if actual != &allowed {
            return Err(format!(
                "dependency boundary violation for {package}: expected {allowed:?}, found {actual:?}"
            ));
        }
    }
    Ok(())
}

fn unsafe_boundary(root: &Path) -> Result<()> {
    let crates = root.join("crates");
    let mut violations = Vec::new();
    visit_files(&crates, &mut |path| {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
            || path.starts_with(root.join("crates/slice-ebpf"))
        {
            return;
        }
        if let Ok(source) = fs::read_to_string(path) {
            if source
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|word| word == "unsafe")
            {
                violations.push(path.display().to_string());
            }
        }
    })?;
    if violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unsafe Rust outside slice-ebpf: {}",
            violations.join(", ")
        ))
    }
}

fn markdown_links(root: &Path) -> Result<()> {
    let mut broken = Vec::new();
    visit_files(root, &mut |path| {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md")
            || path.starts_with(root.join(".git"))
        {
            return;
        }
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };
        let mut rest = source.as_str();
        while let Some(start) = rest.find("](") {
            let target_start = start + 2;
            let Some(end_offset) = rest[target_start..].find(')') else {
                break;
            };
            let target = rest[target_start..target_start + end_offset]
                .trim()
                .trim_matches('<')
                .trim_matches('>');
            let target = target.split('#').next().unwrap_or_default();
            if !target.is_empty()
                && !target.starts_with("http://")
                && !target.starts_with("https://")
                && !target.starts_with("mailto:")
                && !target.starts_with("/")
                && !path.parent().unwrap_or(root).join(target).exists()
            {
                broken.push(format!("{} -> {target}", path.display()));
            }
            rest = &rest[target_start + end_offset + 1..];
        }
    })?;
    if broken.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "broken local Markdown links: {}",
            broken.join(", ")
        ))
    }
}

fn workflow_pins(root: &Path) -> Result<()> {
    let workflows = root.join(".github/workflows");
    if !workflows.exists() {
        return Err(".github/workflows is missing".to_owned());
    }
    let mut unpinned = Vec::new();
    visit_files(&workflows, &mut |path| {
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("yml" | "yaml")
        ) {
            return;
        }
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };
        if source.contains("pull_request_target") {
            unpinned.push(format!(
                "{}: pull_request_target is forbidden",
                path.display()
            ));
        }
        for line in source.lines().filter(|line| line.contains("uses:")) {
            let Some(at) = line.find('@') else {
                unpinned.push(format!("{}: {line}", path.display()));
                continue;
            };
            let reference = line[at + 1..]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|character: char| character == '"' || character == '\'');
            if reference.len() != 40
                || !reference
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                unpinned.push(format!("{}: {line}", path.display()));
            }
        }
    })?;
    if unpinned.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "workflow actions must use 40-character SHAs: {}",
            unpinned.join(", ")
        ))
    }
}

fn visit_files(root: &Path, callback: &mut impl FnMut(&Path)) -> Result<()> {
    let entries =
        fs::read_dir(root).map_err(|error| format!("reading {}: {error}", root.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("reading directory entry: {error}"))?
            .path();
        if path.is_dir() {
            visit_files(&path, callback)?;
        } else {
            callback(&path);
        }
    }
    Ok(())
}
