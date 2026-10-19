use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::Event;

/// Decides which changed paths cause a restart.
pub struct Filter {
    root: PathBuf,
    exts: Vec<String>,
    ignore: Gitignore,
}

impl Filter {
    pub fn new(root: &Path, exts: &[String], patterns: &[String]) -> Result<Self> {
        let mut builder = GitignoreBuilder::new(root);
        // A missing .gitignore is not an error.
        let _ = builder.add(root.join(".gitignore"));
        builder.add_line(None, "/target/")?;
        for pattern in patterns {
            builder.add_line(None, pattern).with_context(|| format!("bad ignore pattern: {pattern}"))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            exts: exts.iter().filter(|e| !e.is_empty()).map(|e| e.trim_start_matches('.').to_owned()).collect(),
            ignore: builder.build()?,
        })
    }

    pub fn wants(&self, path: &Path) -> bool {
        if path.components().any(|c| c == Component::Normal(".git".as_ref())) {
            return false;
        }
        // Editors and tools write temporary files with hidden names, for example `.!123!main.rs`.
        if path.file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b".")) {
            return false;
        }
        if !self.exts.is_empty() {
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return false };
            if !self.exts.iter().any(|e| e == ext) {
                return false;
            }
        }
        // The matcher panics on paths outside the root, so only check paths in it.
        !(path.starts_with(&self.root) && self.ignore.matched_path_or_any_parents(path, false).is_ignore())
    }
}

/// Starts to watch the paths. The watch stops when the returned value is dropped.
pub fn start(paths: &[PathBuf], filter: Filter, tx: Sender<Event>) -> Result<impl Watcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let paths: Vec<PathBuf> = event.paths.into_iter().filter(|p| filter.wants(p)).collect();
        if !paths.is_empty() {
            let _ = tx.send(Event::Changed(paths));
        }
    })?;
    for path in paths {
        watcher.watch(path, RecursiveMode::Recursive).with_context(|| format!("cannot watch {}", path.display()))?;
    }
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(patterns: &[&str]) -> Filter {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        Filter::new(Path::new("/p"), &["rs".into(), "toml".into()], &patterns).unwrap()
    }

    #[test]
    fn keeps_source_files() {
        let f = filter(&[]);
        assert!(f.wants(Path::new("/p/src/main.rs")));
        assert!(f.wants(Path::new("/p/Cargo.toml")));
    }

    #[test]
    fn drops_other_extensions_and_build_output() {
        let f = filter(&[]);
        assert!(!f.wants(Path::new("/p/README.md")));
        assert!(!f.wants(Path::new("/p/src/main.rs~")));
        assert!(!f.wants(Path::new("/p/target/debug/build/x/out.rs")));
        assert!(!f.wants(Path::new("/p/.git/x.toml")));
        assert!(!f.wants(Path::new("/p/src/.!123!main.rs")));
    }

    #[test]
    fn applies_ignore_patterns() {
        let f = filter(&["generated/"]);
        assert!(!f.wants(Path::new("/p/src/generated/a.rs")));
        assert!(f.wants(Path::new("/p/src/a.rs")));
    }

    #[test]
    fn keeps_paths_outside_the_root() {
        assert!(filter(&[]).wants(Path::new("/other/lib.rs")));
    }
}
