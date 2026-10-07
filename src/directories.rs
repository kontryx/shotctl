use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use ratatui::widgets::TableState;

use crate::config::expand_path;

pub enum Completion {
    Completed(String),
    Choices(DirectoryBrowser),
}

pub struct DirectoryBrowser {
    pub current: PathBuf,
    pub entries: Vec<PathBuf>,
    pub state: TableState,
    pub filter: String,
    pub show_hidden: bool,
    pub error: String,
    pub new_folder_input: Option<String>,
    // Keep ~/ and relative prefixes when accepting a completion candidate.
    completion_prefix: Option<String>,
}

fn children(path: &Path, prefix: &str, show_hidden: bool) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(path).with_context(|| format!("Cannot read directory {}", path.display()))?
    {
        let entry = entry.with_context(|| format!("Cannot list {}", path.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(prefix)
            && (show_hidden || !name.starts_with('.') || prefix.starts_with('.'))
            && entry.path().is_dir()
        {
            entries.push(entry.path());
        }
    }
    entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(entries)
}

fn with_separator(input: &str) -> String {
    if input.chars().last().is_some_and(std::path::is_separator) {
        input.to_owned()
    } else {
        format!("{input}{}", std::path::MAIN_SEPARATOR)
    }
}

pub fn complete(input: &str) -> Result<Completion> {
    let input = input.trim();
    if input == "~" {
        return Ok(Completion::Completed("~/".into()));
    }
    let (base, prefix) = input
        .rfind(std::path::is_separator)
        .map(|index| input.split_at(index + 1))
        .unwrap_or(("", input));
    let parent = if base.is_empty() {
        PathBuf::from(".")
    } else {
        expand_path(base)
    };
    let entries = children(&parent, prefix, false)?;
    anyhow::ensure!(
        !entries.is_empty(),
        "No matching folders. Ctrl+B opens the folder browser."
    );
    let names: Vec<_> = entries
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap())
        .collect();
    let common: String = names[0]
        .chars()
        .enumerate()
        .take_while(|(index, ch)| {
            names
                .iter()
                .all(|name| name.chars().nth(*index) == Some(*ch))
        })
        .map(|(_, ch)| ch)
        .collect();
    if names.len() == 1 {
        return Ok(Completion::Completed(with_separator(&format!(
            "{base}{}",
            names[0]
        ))));
    }
    if common.len() > prefix.len() {
        return Ok(Completion::Completed(format!("{base}{common}")));
    }
    Ok(Completion::Choices(DirectoryBrowser {
        current: parent.canonicalize()?,
        state: TableState::default().with_selected(Some(0)),
        entries,
        filter: prefix.to_owned(),
        show_hidden: prefix.starts_with('.'),
        error: String::new(),
        new_folder_input: None,
        completion_prefix: Some(base.to_owned()),
    }))
}

impl DirectoryBrowser {
    pub fn open(input: &str) -> Result<Self> {
        let mut path = if input.trim().is_empty() {
            dirs::home_dir().unwrap_or(std::env::current_dir()?)
        } else {
            expand_path(input)
        };
        if !path.is_absolute() {
            path = std::env::current_dir()?.join(path);
        }
        // Defaults and manually entered paths may contain folders not created yet.
        while !path.is_dir() {
            if !path.pop() {
                anyhow::bail!("Cannot find a directory to browse");
            }
        }
        let current = path.canonicalize()?;
        let entries = children(&current, "", false)?;
        let selected = (!entries.is_empty()).then_some(0);
        Ok(Self {
            current,
            entries,
            state: TableState::default().with_selected(selected),
            filter: String::new(),
            show_hidden: false,
            error: String::new(),
            new_folder_input: None,
            completion_prefix: None,
        })
    }

    pub fn selected(&self) -> Option<&PathBuf> {
        self.state
            .selected()
            .and_then(|index| self.entries.get(index))
    }

    pub fn selected_input(&self) -> Option<String> {
        let selected = self.selected()?;
        Some(with_separator(
            &if let Some(prefix) = &self.completion_prefix {
                format!("{prefix}{}", selected.file_name()?.to_str()?)
            } else {
                selected.to_str()?.to_owned()
            },
        ))
    }

    pub fn navigate(&mut self, path: PathBuf) {
        let result = (|| -> Result<_> {
            let current = path.canonicalize()?;
            let entries = children(&current, "", self.show_hidden)?;
            Ok((current, entries))
        })();
        match result {
            Ok((current, entries)) => {
                let previous = self.current.clone();
                let selected = entries
                    .iter()
                    .position(|entry| entry == &previous)
                    .or_else(|| (!entries.is_empty()).then_some(0));
                self.current = current;
                self.entries = entries;
                self.state = TableState::default().with_selected(selected);
                self.filter.clear();
                self.completion_prefix = None;
                self.error.clear();
            }
            Err(error) => self.error = format!("{error:#}"),
        }
    }

    pub fn create_folder(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "Enter a folder name.");
        let mut components = Path::new(name).components();
        anyhow::ensure!(
            matches!(components.next(), Some(Component::Normal(_)))
                && components.next().is_none()
                && !name.contains(['/', '\\'])
                && !name.chars().any(char::is_control),
            "Enter a single folder name, without path separators."
        );
        let path = self.current.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                anyhow::bail!("A file or folder named '{name}' already exists.");
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot create folder in {}", self.current.display())
                });
            }
        }
        if name.starts_with('.') {
            self.show_hidden = true;
        }
        self.navigate(path);
        Ok(())
    }

    pub fn toggle_hidden(&mut self) {
        let show_hidden = !self.show_hidden;
        match children(&self.current, &self.filter, show_hidden) {
            Ok(entries) => {
                let selected = self.selected().cloned();
                self.state = TableState::default().with_selected(
                    selected
                        .and_then(|path| entries.iter().position(|entry| entry == &path))
                        .or_else(|| (!entries.is_empty()).then_some(0)),
                );
                self.entries = entries;
                self.show_hidden = show_hidden;
                self.error.clear();
            }
            Err(error) => self.error = format!("{error:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn completed(input: &str) -> String {
        match complete(input).unwrap() {
            Completion::Completed(path) => path,
            Completion::Choices(_) => panic!("Expected an unambiguous completion"),
        }
    }

    #[test]
    fn completion_handles_spaces_unicode_and_shared_prefixes() {
        let temp = TempDir::new().unwrap();
        for name in ["Photo Archive", "Photo Albums", "旅行一", "旅行二"] {
            fs::create_dir(temp.path().join(name)).unwrap();
        }
        fs::write(temp.path().join("Photo File"), b"not a folder").unwrap();
        let base = format!("{}/", temp.path().display());
        assert_eq!(
            completed(&format!("{base}Photo Ar")),
            format!("{base}Photo Archive/")
        );
        assert_eq!(completed(&format!("{base}Pho")), format!("{base}Photo A"));
        assert_eq!(completed(&format!("{base}旅")), format!("{base}旅行"));
        let Completion::Choices(mut browser) = complete(&format!("{base}Photo A")).unwrap() else {
            panic!("Expected a choice of folders")
        };
        assert_eq!(browser.entries.len(), 2);
        assert_eq!(
            browser.selected_input().unwrap(),
            format!("{base}Photo Albums/")
        );
        browser.state.select(Some(1));
        assert_eq!(
            browser.selected_input().unwrap(),
            format!("{base}Photo Archive/")
        );
        assert!(complete(&format!("{base}missing")).is_err());
        assert!(complete(&format!("{base}Photo File")).is_err());
        assert!(complete(&format!("{base}missing/child")).is_err());
    }

    #[test]
    fn hidden_folders_are_opt_in_and_tilde_is_preserved() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("Visible")).unwrap();
        fs::create_dir(temp.path().join(".hidden")).unwrap();
        let base = format!("{}/", temp.path().display());
        assert_eq!(completed(&base), format!("{base}Visible/"));
        assert_eq!(completed(&format!("{base}.h")), format!("{base}.hidden/"));
        assert_eq!(completed("~"), "~/");
        let mut browser = DirectoryBrowser::open(&base).unwrap();
        assert_eq!(browser.entries.len(), 1);
        browser.toggle_hidden();
        assert_eq!(browser.entries.len(), 2);
        browser.toggle_hidden();
        assert_eq!(browser.entries.len(), 1);
    }

    #[test]
    fn browser_starts_at_existing_parent_and_recovers_from_navigation_errors() {
        let temp = TempDir::new().unwrap();
        let child = temp.path().join("nested");
        fs::create_dir(&child).unwrap();
        let mut browser =
            DirectoryBrowser::open(child.join("new/archive").to_str().unwrap()).unwrap();
        assert_eq!(browser.current, child.canonicalize().unwrap());
        assert!(browser.entries.is_empty());
        browser.navigate(temp.path().to_owned());
        assert_eq!(browser.selected().unwrap(), &child.canonicalize().unwrap());
        let previous = browser.current.clone();
        browser.navigate(temp.path().join("missing"));
        assert!(!browser.error.is_empty());
        assert_eq!(browser.current, previous);
        browser.navigate(child);
        assert!(browser.error.is_empty());
        assert!(browser.entries.is_empty());
        assert!(browser.selected().is_none());
    }

    #[test]
    fn new_folders_are_created_inside_the_current_directory_without_replacing_entries() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut browser = DirectoryBrowser::open(root.to_str().unwrap()).unwrap();
        fs::write(root.join("existing"), b"keep this file").unwrap();
        fs::create_dir(root.join("existing-folder")).unwrap();
        for name in [
            "",
            "  ",
            ".",
            "..",
            "../outside",
            "nested/path",
            "nested\\path",
            "/absolute",
            "bad\0name",
            "existing",
            "existing-folder",
        ] {
            assert!(
                browser.create_folder(name).is_err(),
                "Should reject: {name:?}"
            );
            assert_eq!(browser.current, root);
        }
        assert_eq!(fs::read(root.join("existing")).unwrap(), b"keep this file");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        browser.create_folder("New Photo Archive 旅行").unwrap();
        assert_eq!(browser.current, root.join("New Photo Archive 旅行"));
        assert!(browser.current.is_dir());
        assert!(browser.entries.is_empty());
        browser.navigate(root.clone());
        browser.create_folder(".hidden-archive").unwrap();
        assert_eq!(browser.current, root.join(".hidden-archive"));
        browser.navigate(root.clone());
        assert!(browser.show_hidden);
        assert!(browser.entries.contains(&root.join(".hidden-archive")));
        browser.navigate(root.join("missing-parent"));
        assert!(!browser.error.is_empty());
    }
}
