//! Windows 7-inspired Explorer presentation helpers.
//!
//! Keep navigation semantics independent of the visual shell: breadcrumb
//! labels are view data, while the actual destinations remain Linux paths.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub path: PathBuf,
}

pub fn breadcrumbs(path: &Path, home: &Path) -> Vec<Crumb> {
    if path == home || path.starts_with(home) {
        let mut result = vec![Crumb {
            label: "Home".into(),
            path: home.to_path_buf(),
        }];
        let mut destination = home.to_path_buf();
        if let Ok(relative) = path.strip_prefix(home) {
            for component in relative.components() {
                if let Component::Normal(name) = component {
                    destination.push(name);
                    result.push(Crumb {
                        label: name.to_string_lossy().into_owned(),
                        path: destination.clone(),
                    });
                }
            }
        }
        return result;
    }

    let mut result = vec![Crumb {
        label: "Computer".into(),
        path: PathBuf::from("/"),
    }];
    let mut destination = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(name) = component {
            destination.push(name);
            result.push(Crumb {
                label: name.to_string_lossy().into_owned(),
                path: destination.clone(),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_crumbs_are_navigable_without_fake_windows_paths() {
        let crumbs = breadcrumbs(
            Path::new("/home/alex/My Documents/2026"),
            Path::new("/home/alex"),
        );
        assert_eq!(
            crumbs
                .iter()
                .map(|crumb| crumb.label.as_str())
                .collect::<Vec<_>>(),
            ["Home", "My Documents", "2026"]
        );
        assert_eq!(crumbs[1].path, Path::new("/home/alex/My Documents"));
    }

    #[test]
    fn external_paths_keep_computer_root_and_every_parent() {
        let crumbs = breadcrumbs(Path::new("/mnt/games"), Path::new("/home/alex"));
        assert_eq!(
            crumbs
                .iter()
                .map(|crumb| crumb.label.as_str())
                .collect::<Vec<_>>(),
            ["Computer", "mnt", "games"]
        );
        assert_eq!(crumbs[0].path, Path::new("/"));
        assert_eq!(crumbs.last().unwrap().path, Path::new("/mnt/games"));
        assert_eq!(
            breadcrumbs(Path::new("/"), Path::new("/home/alex")).len(),
            1
        );
    }
}
