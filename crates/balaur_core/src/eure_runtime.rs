//! One incremental Eure query runtime per [`Engine`](crate::engine::Engine).
//!
//! File parses go through `query_flow`'s dependency tracking rather than a
//! one-shot `eure::parse_content` call: a prefab instantiated many times in
//! one scene, or a theme file read on every reload, reparses only when its
//! content actually changed since the runtime last saw it.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use anyhow::{bail, Result};
use eure::query::{ParseEure, TextFile, TextFileContent, WithFormattedError};
use eure::query_flow::DurabilityLevel;
use eure::report::IntoErrorReports;
use eure::FromEure;

use crate::eure_value::EureValue;

pub struct EureRuntime {
    runtime: eure::query_flow::QueryRuntime,
}

impl Default for EureRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl EureRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: eure::query::build_runtime(),
        }
    }

    /// Parses `path`'s `content` as `T`.
    ///
    /// `path` is only a cache key and a label for error messages; `content`
    /// is always used as given rather than re-read from disk, so a caller
    /// serving a packed or in-memory file works exactly like one reading the
    /// project directory.
    ///
    /// `T` must be real `PartialEq`, not a stand-in that always reports
    /// equal: query-flow backdates a query's result to the previously cached
    /// value whenever a recompute compares equal to it, at the query's own
    /// level and not only for downstream queries, so an always-equal wrapper
    /// would make every reparse after the first return the first file's
    /// content forever.
    pub fn parse<T>(&self, path: &Path, content: &str) -> Result<T>
    where
        T: for<'doc> FromEure<'doc> + Clone + PartialEq + Send + Sync + 'static,
        for<'doc> <T as FromEure<'doc>>::Error: IntoErrorReports,
    {
        let file = TextFile::from_path(path.to_path_buf());
        self.runtime.resolve_asset(
            file.clone(),
            TextFileContent(content.to_string()),
            DurabilityLevel::Volatile,
        );
        let result = self
            .runtime
            .query(WithFormattedError::new(ParseEure::<T>::new(file), false))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        match &*result {
            Ok(value) => Ok((**value).clone()),
            Err(message) => bail!("{message}"),
        }
    }

    /// Parses `path`'s `content` as a dynamic [`EureValue`].
    pub fn parse_value(&self, path: &Path, content: &str) -> Result<EureValue> {
        self.parse::<EureValue>(path, content)
    }
}

/// The engine's shared incremental Eure runtime, created on first use.
///
/// Every project/scene/theme/animation/component/asset file the engine loads
/// goes through this one runtime for that engine's whole lifetime, so a
/// prefab referenced by many nodes or a schema shared by many components
/// parses once per content, not once per reference.
pub fn of(eng: &crate::engine::Engine) -> Rc<RefCell<EureRuntime>> {
    if let Some(runtime) = eng.try_resource::<EureRuntime>() {
        return runtime;
    }
    eng.insert_resource(EureRuntime::new());
    eng.resource::<EureRuntime>()
}
