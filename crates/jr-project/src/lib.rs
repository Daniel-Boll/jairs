//! Project discovery and the immutable catalog of importable modules.
//!
//! This crate is the boundary between filesystem/configuration state and the compiler's
//! incremental database. It performs every directory walk and source read before a catalog is
//! installed as a salsa input; consumers therefore ask one in-memory question instead of
//! reimplementing search order or performing untracked I/O.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

/// A request to discover the project governing one source or directory.
#[derive(Debug, Clone)]
pub struct DiscoverRequest {
    /// A source file or directory from which to find the governing `jairs.toml`.
    pub anchor: PathBuf,
    /// Ordered compatibility roots supplied by the operator (`-I`).
    pub operator_roots: Vec<PathBuf>,
}

impl DiscoverRequest {
    /// Creates a discovery request with no compatibility roots.
    #[must_use]
    pub fn new(anchor: impl Into<PathBuf>) -> Self {
        Self {
            anchor: anchor.into(),
            operator_roots: Vec::new(),
        }
    }

    /// Adds ordered operator roots.
    #[must_use]
    pub fn with_operator_roots(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.operator_roots.extend(roots);
        self
    }
}

/// Why a module is present in a catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleOrigin {
    /// An implicit direct child of a manifest-backed project's `src/`.
    ProjectLocal,
    /// An exact named dependency from `[dependencies]`.
    ExactDependency,
    /// A compatibility root supplied with `-I`.
    OperatorPath,
    /// A compatibility root from `[build].module_paths`.
    LegacyPath,
    /// A compatibility root added by one build-script target.
    BuildPath,
    /// A module compiled into the `jr` binary.
    Bundled,
    /// A compiler-generated module added for one build target.
    Generated,
    /// A build script's exact `provide_import` result.
    Provided,
}

impl std::fmt::Display for ModuleOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ProjectLocal => "project-local module",
            Self::ExactDependency => "exact dependency",
            Self::OperatorPath => "operator module path",
            Self::LegacyPath => "legacy manifest module path",
            Self::BuildPath => "build-target module path",
            Self::Bundled => "bundled standard-library module",
            Self::Generated => "generated module",
            Self::Provided => "provided import",
        })
    }
}

/// One exact module source, ready to install without more filesystem I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntry {
    name: Arc<str>,
    path: PathBuf,
    text: Arc<str>,
    origin: ModuleOrigin,
}

impl ModuleEntry {
    /// Creates an already-read exact source.
    ///
    /// This is the installation seam for virtual files and test files. Normal filesystem callers
    /// should use [`ProjectContext::discover`] so precedence and collision policy are applied once.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidName`] when `name` is not flat.
    pub fn from_text(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        text: impl Into<Arc<str>>,
        origin: ModuleOrigin,
    ) -> Result<Self, Error> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self {
            name: Arc::from(name),
            path: path.into(),
            text: text.into(),
            origin,
        })
    }

    /// The flat name used by `#import`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The source's stable display/identity path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The source text captured during discovery.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Why this entry exists.
    #[must_use]
    pub fn origin(&self) -> ModuleOrigin {
        self.origin
    }
}

/// An immutable, sorted snapshot of every importable module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleCatalog {
    entries: Arc<[ModuleEntry]>,
    probe_roots: Arc<[PathBuf]>,
}

impl ModuleCatalog {
    /// Creates a catalog from already-resolved exact entries.
    ///
    /// Entries are sorted and duplicate names are rejected. This deliberately applies no
    /// precedence policy: it is for adapters that have already resolved one winner per name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Duplicate`] when two entries have the same import name.
    pub fn from_entries(
        entries: impl IntoIterator<Item = ModuleEntry>,
        probe_roots: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, Error> {
        let mut unique = BTreeMap::new();
        for entry in entries {
            insert_unique(&mut unique, entry)?;
        }
        Ok(Self {
            entries: unique.into_values().collect::<Vec<_>>().into(),
            probe_roots: probe_roots.into_iter().collect::<Vec<_>>().into(),
        })
    }

    /// Every entry, sorted by import name.
    #[must_use]
    pub fn entries(&self) -> &[ModuleEntry] {
        &self.entries
    }

    /// Looks up one flat import name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ModuleEntry> {
        self.entries
            .binary_search_by(|entry| entry.name().cmp(name))
            .ok()
            .map(|index| &self.entries[index])
    }

    /// Compatibility roots retained only so E0210 can explain where a missing name was sought.
    #[must_use]
    pub fn probe_roots(&self) -> &[PathBuf] {
        &self.probe_roots
    }
}

/// Project facts shared by batch commands, build targets, and the language server.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    root: Option<PathBuf>,
    entry: Option<PathBuf>,
    default_output: Option<PathBuf>,
    owned_roots: Arc<[PathBuf]>,
    catalog: ModuleCatalog,
}

impl ProjectContext {
    /// Discovers the project and reads all module sources.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed manifest, an unreadable declared source, an invalid flat
    /// module name, or an ambiguous/forbidden duplicate.
    pub fn discover(request: DiscoverRequest) -> Result<Self, Error> {
        let located = jr_manifest::find(&request.anchor).map_err(Error::Manifest)?;
        let root = located.as_ref().map(|manifest| manifest.root.clone());
        let entry = located.as_ref().map(jr_manifest::Located::entry);
        let default_output = located.as_ref().and_then(|located| {
            located
                .manifest
                .project
                .name
                .as_ref()
                .map(|name| located.root.join(name))
        });
        let owned_roots: Arc<[PathBuf]> = root
            .clone()
            .or_else(|| anchor_directory(&request.anchor))
            .into_iter()
            .collect::<Vec<_>>()
            .into();

        let operator_roots = absolutize_roots(&request.anchor, request.operator_roots);
        let mut builder = CatalogBuilder::default();
        builder.probe_roots.extend(operator_roots.iter().cloned());

        // Operator paths preserve their historical first-hit rule and override lower tiers.
        for root in &operator_roots {
            for entry in discover_root(
                root,
                ModuleOrigin::OperatorPath,
                RootDuplicates::DirectoryThenFile,
            )? {
                builder
                    .operator
                    .entry(entry.name.to_string())
                    .or_insert(entry);
            }
        }

        if let Some(located) = &located {
            let local_root = located.root.join("src");
            for module in discover_root(
                &local_root,
                ModuleOrigin::ProjectLocal,
                RootDuplicates::Error,
            )? {
                if Some(module.path()) == entry.as_deref() {
                    continue;
                }
                insert_unique(&mut builder.local, module)?;
            }

            for (name, path) in located.exact_dependencies() {
                let entry = read_exact(name, &path, ModuleOrigin::ExactDependency)?;
                insert_unique(&mut builder.exact, entry)?;
            }

            let legacy_roots = located.module_paths();
            builder.probe_roots.extend(legacy_roots.iter().cloned());
            for root in legacy_roots {
                for entry in discover_root(
                    &root,
                    ModuleOrigin::LegacyPath,
                    RootDuplicates::DirectoryThenFile,
                )? {
                    builder
                        .legacy
                        .entry(entry.name.to_string())
                        .or_insert(entry);
                }
            }
        }

        let catalog = builder.finish()?;
        Ok(Self {
            root,
            entry,
            default_output,
            owned_roots,
            catalog,
        })
    }

    /// Derives a target-specific context with exact generated/provided modules.
    ///
    /// # Errors
    ///
    /// Returns an error when an addition cannot be read or its name is already present.
    pub fn derive(&self, additions: ModuleAdditions) -> Result<Self, Error> {
        let mut entries: BTreeMap<String, ModuleEntry> = self
            .catalog
            .entries()
            .iter()
            .cloned()
            .map(|entry| (entry.name.to_string(), entry))
            .collect();
        for addition in additions.entries {
            validate_name(&addition.name)?;
            let entry = read_exact(&addition.name, &addition.path, addition.origin)?;
            if let Some(previous) = entries.get(entry.name()) {
                return Err(Error::Duplicate {
                    name: entry.name.to_string(),
                    first: previous.path.clone(),
                    first_origin: previous.origin,
                    second: entry.path,
                    second_origin: entry.origin,
                });
            }
            entries.insert(entry.name.to_string(), entry);
        }
        Ok(Self {
            root: self.root.clone(),
            entry: self.entry.clone(),
            default_output: self.default_output.clone(),
            owned_roots: Arc::clone(&self.owned_roots),
            catalog: ModuleCatalog {
                entries: entries.into_values().collect::<Vec<_>>().into(),
                probe_roots: Arc::clone(&self.catalog.probe_roots),
            },
        })
    }

    /// Derives a target context with ordered build-script compatibility roots.
    ///
    /// The roots preserve the old `Build_Options.module_paths` contract: the first matching root
    /// wins, and these target-specific paths override the inherited project catalog. Discovery is
    /// still completed here, before the returned catalog is installed into salsa.
    ///
    /// # Errors
    ///
    /// Returns an error when a root is unreadable or contains an invalid/ambiguous module spelling.
    pub fn derive_module_roots(
        &self,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, Error> {
        let roots: Vec<PathBuf> = roots.into_iter().collect();
        let mut overrides = BTreeMap::new();
        for root in &roots {
            for entry in discover_root(
                root,
                ModuleOrigin::BuildPath,
                RootDuplicates::DirectoryThenFile,
            )? {
                overrides.entry(entry.name.to_string()).or_insert(entry);
            }
        }
        let mut entries: BTreeMap<String, ModuleEntry> = self
            .catalog
            .entries()
            .iter()
            .cloned()
            .map(|entry| (entry.name.to_string(), entry))
            .collect();
        entries.extend(overrides);

        let mut probe_roots = roots;
        probe_roots.extend(self.catalog.probe_roots.iter().cloned());
        Ok(Self {
            root: self.root.clone(),
            entry: self.entry.clone(),
            default_output: self.default_output.clone(),
            owned_roots: Arc::clone(&self.owned_roots),
            catalog: ModuleCatalog {
                entries: entries.into_values().collect::<Vec<_>>().into(),
                probe_roots: probe_roots.into(),
            },
        })
    }

    /// The governing manifest directory, if one exists.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The manifest's effective entry point, if one exists.
    #[must_use]
    pub fn entry(&self) -> Option<&Path> {
        self.entry.as_deref()
    }

    /// The manifest-derived fallback artefact path, if the project has a name.
    #[must_use]
    pub fn default_output(&self) -> Option<&Path> {
        self.default_output.as_deref()
    }

    /// Roots whose files belong to this project for workspace-edit operations.
    #[must_use]
    pub fn owned_roots(&self) -> &[PathBuf] {
        &self.owned_roots
    }

    /// Every source available to `#import`.
    #[must_use]
    pub fn catalog(&self) -> &ModuleCatalog {
        &self.catalog
    }
}

/// Exact target-specific modules added to an existing context.
#[derive(Debug, Clone, Default)]
pub struct ModuleAdditions {
    entries: Vec<Addition>,
}

impl ModuleAdditions {
    /// Adds one compiler-generated exact module.
    #[must_use]
    pub fn generated(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.entries.push(Addition {
            name: name.into(),
            path: path.into(),
            origin: ModuleOrigin::Generated,
        });
        self
    }

    /// Adds one exact import supplied by a build script.
    #[must_use]
    pub fn provided(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.entries.push(Addition {
            name: name.into(),
            path: path.into(),
            origin: ModuleOrigin::Provided,
        });
        self
    }
}

#[derive(Debug, Clone)]
struct Addition {
    name: String,
    path: PathBuf,
    origin: ModuleOrigin,
}

/// Why project discovery failed.
#[derive(Debug)]
pub enum Error {
    /// The governing manifest exists but cannot be read or parsed.
    Manifest(jr_manifest::Error),
    /// A module source or directory could not be read.
    Read {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// A declared exact module did not name a file or a directory containing `module.jr`.
    MissingExact {
        /// The import name.
        name: String,
        /// The resolved path.
        path: PathBuf,
    },
    /// An import name was empty or not flat.
    InvalidName {
        /// The rejected name.
        name: String,
    },
    /// Two project declarations supplied one flat import name.
    Duplicate {
        /// The ambiguous import name.
        name: String,
        /// The first source.
        first: PathBuf,
        /// The first source's origin.
        first_origin: ModuleOrigin,
        /// The second source.
        second: PathBuf,
        /// The second source's origin.
        second_origin: ModuleOrigin,
    },
    /// An implicit or legacy source attempted to replace a bundled module.
    BundledShadow {
        /// The standard-library module name.
        name: String,
        /// The source that attempted the shadow.
        path: PathBuf,
        /// Why that source was discovered.
        origin: ModuleOrigin,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifest(error) => error.fmt(f),
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::MissingExact { name, path } => write!(
                f,
                "exact dependency `{name}` names {}, which is neither a `.jr` file nor a directory containing `module.jr`",
                path.display()
            ),
            Self::InvalidName { name } => {
                write!(f, "`{name}` is not a flat module name")
            }
            Self::Duplicate {
                name,
                first,
                first_origin,
                second,
                second_origin,
            } => write!(
                f,
                "module `{name}` is declared twice: {} ({first_origin}) and {} ({second_origin})",
                first.display(),
                second.display()
            ),
            Self::BundledShadow { name, path, origin } => write!(
                f,
                "{} ({origin}) would implicitly shadow bundled module `{name}`; declare an exact dependency or use `-I` to override it explicitly",
                path.display()
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Manifest(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Default)]
struct CatalogBuilder {
    operator: BTreeMap<String, ModuleEntry>,
    local: BTreeMap<String, ModuleEntry>,
    exact: BTreeMap<String, ModuleEntry>,
    legacy: BTreeMap<String, ModuleEntry>,
    probe_roots: Vec<PathBuf>,
}

impl CatalogBuilder {
    fn finish(self) -> Result<ModuleCatalog, Error> {
        for (name, local) in &self.local {
            if let Some(exact) = self.exact.get(name) {
                return Err(duplicate(local, exact));
            }
        }

        let bundled_names: BTreeSet<&str> = jr_stdlib::names().collect();
        for entry in self.local.values().chain(self.legacy.values()) {
            if bundled_names.contains(entry.name()) {
                return Err(Error::BundledShadow {
                    name: entry.name.to_string(),
                    path: entry.path.clone(),
                    origin: entry.origin,
                });
            }
        }

        for (name, legacy) in &self.legacy {
            if let Some(project) = self.local.get(name).or_else(|| self.exact.get(name)) {
                return Err(duplicate(project, legacy));
            }
        }

        let mut entries = self.operator;
        for (name, entry) in self.local.into_iter().chain(self.exact) {
            entries.entry(name).or_insert(entry);
        }
        for (name, entry) in self.legacy {
            entries.entry(name).or_insert(entry);
        }
        for name in jr_stdlib::names() {
            if entries.contains_key(name) {
                continue;
            }
            let path = jr_stdlib::root().join(name).join("module.jr");
            let text = jr_stdlib::source(&path).expect("every bundled name resolves");
            entries.insert(
                name.to_owned(),
                ModuleEntry {
                    name: Arc::from(name),
                    path,
                    text: Arc::from(text),
                    origin: ModuleOrigin::Bundled,
                },
            );
        }

        let mut probe_roots = self.probe_roots;
        if !probe_roots.iter().any(|root| root == jr_stdlib::root()) {
            probe_roots.push(jr_stdlib::root().to_path_buf());
        }
        Ok(ModuleCatalog {
            entries: entries.into_values().collect::<Vec<_>>().into(),
            probe_roots: probe_roots.into(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum RootDuplicates {
    Error,
    DirectoryThenFile,
}

fn discover_root(
    root: &Path,
    origin: ModuleOrigin,
    duplicates: RootDuplicates,
) -> Result<Vec<ModuleEntry>, Error> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(Error::Read {
                path: root.to_path_buf(),
                source,
            });
        }
    };

    let mut directory_form = Vec::new();
    let mut file_form = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        if file_type.is_file() && path.extension().is_some_and(|extension| extension == "jr") {
            let Some(name) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            file_form.push(read_named(&name, path, origin)?);
        } else if file_type.is_dir() {
            let module = path.join("module.jr");
            if module.is_file() {
                let Some(name) = path.file_name().and_then(|component| component.to_str()) else {
                    continue;
                };
                directory_form.push(read_named(name, module, origin)?);
            }
        }
    }
    directory_form.sort_by(|left, right| left.name.cmp(&right.name));
    file_form.sort_by(|left, right| left.name.cmp(&right.name));

    let mut unique = BTreeMap::new();
    for entry in directory_form.into_iter().chain(file_form) {
        match duplicates {
            RootDuplicates::Error => insert_unique(&mut unique, entry)?,
            RootDuplicates::DirectoryThenFile => {
                unique.entry(entry.name.to_string()).or_insert(entry);
            }
        }
    }
    Ok(unique.into_values().collect())
}

fn read_exact(name: &str, path: &Path, origin: ModuleOrigin) -> Result<ModuleEntry, Error> {
    validate_name(name)?;
    let source = if path.is_file() && path.extension().is_some_and(|extension| extension == "jr") {
        path.to_path_buf()
    } else {
        let module = path.join("module.jr");
        if !module.is_file() {
            return Err(Error::MissingExact {
                name: name.to_owned(),
                path: path.to_path_buf(),
            });
        }
        module
    };
    read_named(name, source, origin)
}

fn read_named(name: &str, path: PathBuf, origin: ModuleOrigin) -> Result<ModuleEntry, Error> {
    validate_name(name)?;
    let text = std::fs::read_to_string(&path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    Ok(ModuleEntry {
        name: Arc::from(name),
        path,
        text: Arc::from(text),
        origin,
    })
}

fn insert_unique(
    entries: &mut BTreeMap<String, ModuleEntry>,
    entry: ModuleEntry,
) -> Result<(), Error> {
    if let Some(previous) = entries.get(entry.name()) {
        return Err(duplicate(previous, &entry));
    }
    entries.insert(entry.name.to_string(), entry);
    Ok(())
}

fn duplicate(first: &ModuleEntry, second: &ModuleEntry) -> Error {
    Error::Duplicate {
        name: first.name.to_string(),
        first: first.path.clone(),
        first_origin: first.origin,
        second: second.path.clone(),
        second_origin: second.origin,
    }
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(Error::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn absolutize_roots(anchor: &Path, roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let base = anchor_directory(anchor).unwrap_or_else(|| PathBuf::from("."));
    roots
        .into_iter()
        .map(|root| {
            if root.is_absolute() {
                root
            } else {
                base.join(root)
            }
        })
        .collect()
}

fn anchor_directory(anchor: &Path) -> Option<PathBuf> {
    if anchor.is_dir() {
        Some(anchor.to_path_buf())
    } else {
        anchor.parent().map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{DiscoverRequest, Error, ModuleAdditions, ModuleOrigin, ProjectContext};

    fn write(path: &std::path::Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, text).expect("write source");
    }

    #[test]
    fn manifest_src_supports_both_flat_spellings() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            &temp.path().join("jairs.toml"),
            "[project]\nname = \"demo\"\n",
        );
        write(&temp.path().join("src/Flat.jr"), "flat :: 1;\n");
        write(&temp.path().join("src/Nested/module.jr"), "nested :: 2;\n");

        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        assert_eq!(
            context.catalog().get("Flat").map(|entry| entry.origin()),
            Some(ModuleOrigin::ProjectLocal)
        );
        assert_eq!(
            context.catalog().get("Nested").map(|entry| entry.origin()),
            Some(ModuleOrigin::ProjectLocal)
        );
    }

    #[test]
    fn implicit_src_requires_a_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("src/Hidden.jr"), "hidden :: 1;\n");

        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        assert!(context.catalog().get("Hidden").is_none());
        assert!(context.catalog().get("Basic").is_some());
    }

    #[test]
    fn the_project_entry_is_not_also_an_importable_module() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("jairs.toml"), "");
        write(&temp.path().join("src/main.jr"), "main :: () {}\n");
        write(&temp.path().join("src/Helper.jr"), "helper :: 1;\n");

        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        assert!(context.catalog().get("main").is_none());
        assert!(context.catalog().get("Helper").is_some());
    }

    #[test]
    fn both_local_spellings_are_an_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("jairs.toml"), "");
        write(&temp.path().join("src/Same.jr"), "a :: 1;\n");
        write(&temp.path().join("src/Same/module.jr"), "b :: 2;\n");

        let error =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect_err("duplicate");
        assert!(matches!(error, Error::Duplicate { name, .. } if name == "Same"));
    }

    #[test]
    fn implicit_local_cannot_shadow_the_standard_library() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("jairs.toml"), "");
        write(&temp.path().join("src/Basic.jr"), "not_basic :: 1;\n");

        let error =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect_err("shadow");
        assert!(matches!(error, Error::BundledShadow { name, .. } if name == "Basic"));
    }

    #[test]
    fn exact_dependencies_name_one_file_or_one_module_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            &temp.path().join("jairs.toml"),
            "[dependencies]\n\
             Geometry = { path = \"external/geometry\" }\n\
             Noise = { path = \"vendor/noise.jr\" }\n",
        );
        write(
            &temp.path().join("external/geometry/module.jr"),
            "point :: 1;\n",
        );
        write(&temp.path().join("vendor/noise.jr"), "noise :: 2;\n");
        write(
            &temp.path().join("external/geometry/Unrelated.jr"),
            "unrelated :: 3;\n",
        );

        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        assert_eq!(
            context
                .catalog()
                .get("Geometry")
                .map(|entry| entry.origin()),
            Some(ModuleOrigin::ExactDependency)
        );
        assert_eq!(
            context.catalog().get("Noise").map(|entry| entry.origin()),
            Some(ModuleOrigin::ExactDependency)
        );
        assert!(
            context.catalog().get("Unrelated").is_none(),
            "an exact directory dependency must not expose siblings"
        );
    }

    #[test]
    fn an_exact_dependency_can_explicitly_override_the_standard_library() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            &temp.path().join("jairs.toml"),
            "[dependencies]\nBasic = { path = \"replacement.jr\" }\n",
        );
        write(&temp.path().join("replacement.jr"), "replacement :: 1;\n");

        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        let basic = context.catalog().get("Basic").expect("Basic");
        assert_eq!(basic.origin(), ModuleOrigin::ExactDependency);
        assert!(basic.text().contains("replacement"));
    }

    #[test]
    fn operator_path_explicitly_overrides_the_standard_library() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            &temp.path().join("override/Basic.jr"),
            "replacement :: 1;\n",
        );

        let context = ProjectContext::discover(
            DiscoverRequest::new(temp.path()).with_operator_roots([temp.path().join("override")]),
        )
        .expect("discover");
        let basic = context.catalog().get("Basic").expect("Basic");
        assert_eq!(basic.origin(), ModuleOrigin::OperatorPath);
        assert!(basic.text().contains("replacement"));
    }

    #[test]
    fn a_compatibility_root_keeps_directory_form_before_file_form() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("mods/Same.jr"), "FILE :: 1;\n");
        write(
            &temp.path().join("mods/Same/module.jr"),
            "DIRECTORY :: 2;\n",
        );

        let context = ProjectContext::discover(
            DiscoverRequest::new(temp.path()).with_operator_roots([temp.path().join("mods")]),
        )
        .expect("discover");
        let same = context.catalog().get("Same").expect("Same");
        assert!(same.text().contains("DIRECTORY"));
        assert!(!same.text().contains("FILE"));
    }

    #[test]
    fn derived_modules_are_exact_and_reject_duplicates() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(&temp.path().join("generated.jr"), "generated :: 1;\n");
        let context =
            ProjectContext::discover(DiscoverRequest::new(temp.path())).expect("discover");
        let derived = context
            .derive(ModuleAdditions::default().generated("Build", temp.path().join("generated.jr")))
            .expect("derive");
        assert_eq!(
            derived.catalog().get("Build").map(|entry| entry.origin()),
            Some(ModuleOrigin::Generated)
        );

        let error = derived
            .derive(ModuleAdditions::default().provided("Build", temp.path().join("generated.jr")))
            .expect_err("duplicate");
        assert!(matches!(error, Error::Duplicate { name, .. } if name == "Build"));
    }
}
