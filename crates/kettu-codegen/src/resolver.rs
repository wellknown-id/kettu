//! File resolver for Kettu imports
//!
//! Resolves package paths to filesystem paths.

use kettu_parser::{UsePath, WitFile};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Resolved imports from a file
pub struct ResolvedImports {
    /// Map of interface alias (e.g., "math") to file path and interface name
    pub imports: HashMap<String, (PathBuf, String)>,
}

/// Resolve all imports in a file relative to its location
pub fn resolve_imports(file_path: &Path, ast: &WitFile) -> ResolvedImports {
    let base_dir = file_path.parent().unwrap_or(Path::new("."));
    let mut imports = HashMap::new();

    for item in &ast.items {
        match item {
            kettu_parser::TopLevelItem::Use(use_stmt) => {
                if let Some(resolved) = resolve_use_path(base_dir, file_path, &use_stmt.path) {
                    let alias = use_stmt
                        .alias
                        .as_ref()
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| use_stmt.path.interface.name.clone());
                    imports.insert(alias, resolved);
                }
            }
            kettu_parser::TopLevelItem::World(world) => {
                for world_item in &world.items {
                    match world_item {
                        kettu_parser::WorldItem::Import(import_export)
                        | kettu_parser::WorldItem::Export(import_export) => {
                            if let kettu_parser::ImportExportKind::Path(path) = &import_export.kind
                            {
                                if let Some(resolved) = resolve_use_path(base_dir, file_path, path)
                                {
                                    imports
                                        .entry(path.interface.name.clone())
                                        .or_insert(resolved);
                                }
                            }
                        }
                        kettu_parser::WorldItem::Include(include_stmt) => {
                            if let Some(resolved) =
                                resolve_use_path(base_dir, file_path, &include_stmt.path)
                            {
                                imports
                                    .entry(include_stmt.path.interface.name.clone())
                                    .or_insert(resolved);
                            }
                        }
                        kettu_parser::WorldItem::Use(use_stmt) => {
                            if let Some(resolved) =
                                resolve_use_path(base_dir, file_path, &use_stmt.path)
                            {
                                imports
                                    .entry(use_stmt.path.interface.name.clone())
                                    .or_insert(resolved);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    ResolvedImports { imports }
}

/// Convert a use path to a file path and interface name
fn resolve_use_path(
    base_dir: &Path,
    current_file: &Path,
    use_path: &UsePath,
) -> Option<(PathBuf, String)> {
    let interface_name = use_path.interface.name.clone();

    if let Some(pkg) = &use_path.package {
        // pkg:name/interface -> ./pkg/name.kettu with interface "interface"
        let mut path = base_dir.to_path_buf();

        // Add namespace parts as directory
        for ns in &pkg.namespace {
            path.push(&ns.name);
        }

        // Add name as filename
        if let Some(name) = pkg.name.first() {
            path.push(format!("{}.kettu", name.name));
        }

        Some((path, interface_name))
    } else {
        resolve_local_interface(base_dir, current_file, &interface_name)
            .map(|path| (path, interface_name))
    }
}

fn resolve_local_interface(
    base_dir: &Path,
    current_file: &Path,
    interface_name: &str,
) -> Option<PathBuf> {
    find_interface_in_tree(base_dir, current_file, interface_name)
}

fn find_interface_in_tree(
    base_dir: &Path,
    current_file: &Path,
    interface_name: &str,
) -> Option<PathBuf> {
    let entries = fs::read_dir(base_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_interface_in_tree(&path, current_file, interface_name) {
                return Some(found);
            }
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("kettu") | Some("wit")) {
            continue;
        }

        if path == current_file {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let (ast, errors) = kettu_parser::parse_file(&content);
        if !errors.is_empty() {
            continue;
        }

        let Some(ast) = ast else {
            continue;
        };

        let has_interface = ast.items.iter().any(|item| {
            matches!(
                item,
                kettu_parser::TopLevelItem::Interface(iface) if iface.name.name == interface_name
            )
        });

        if has_interface {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kettu_parser::PackagePath;

    #[test]
    fn test_resolve_package_path() {
        let base = Path::new("/project/src");
        let use_path = UsePath {
            package: Some(PackagePath {
                namespace: vec![kettu_parser::Id {
                    name: "helper".to_string(),
                    span: 0..0,
                }],
                name: vec![kettu_parser::Id {
                    name: "lib".to_string(),
                    span: 0..0,
                }],
                version: None,
            }),
            interface: kettu_parser::Id {
                name: "math".to_string(),
                span: 0..0,
            },
        };

        let (path, interface) =
            resolve_use_path(base, Path::new("/project/src/main.kettu"), &use_path).unwrap();
        assert_eq!(path, PathBuf::from("/project/src/helper/lib.kettu"));
        assert_eq!(interface, "math");
    }
}
