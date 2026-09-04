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
use eure::document::parse::ParseContext;
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
    pub fn parse<T>(&self, path: &Path, content: &str) -> Result<T>
    where
        T: for<'doc> FromEure<'doc> + Clone + Send + Sync + 'static,
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
            .query(WithFormattedError::new(
                ParseEure::<AlwaysEq<T>>::new(file),
                false,
            ))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        match &*result {
            Ok(value) => Ok((**value).clone().0),
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

/// Wraps a `FromEure` output so it can be a query result without requiring
/// `PartialEq` on the wrapped type: query-flow only uses the comparison to
/// decide whether *downstream* queries can skip recomputing, and we have no
/// downstream queries here, only the cached parse itself (which is keyed on
/// the input content, not on this comparison).
#[derive(Clone)]
struct AlwaysEq<T>(T);

impl<T> PartialEq for AlwaysEq<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<'doc, T: FromEure<'doc>> FromEure<'doc> for AlwaysEq<T> {
    type Error = T::Error;

    fn parse(ctx: &ParseContext<'doc>) -> Result<Self, Self::Error> {
        T::parse(ctx).map(AlwaysEq)
    }
}
