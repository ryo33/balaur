//! A dynamic Eure value: the replacement for the old `toml::Value`/`toml::Table`
//! bag that scene keys, component schemas, asset definitions and script
//! properties used to pass around.
//!
//! Backed directly by [`eure::document::EureDocument`] — an `EureValue` owns a
//! self-contained document rooted at the value it represents. Navigating into
//! a child (`get`, `iter_table`, `iter_array`) extracts that child's subtree
//! as its own document via [`EureDocument::node_subtree_to_document`], so an
//! `EureValue` is always a standalone value, never a borrow into a bigger one.
//!
//! `FromEure`/`IntoEure` delegate straight to `EureDocument`'s own impls, so
//! this type drops into any `#[derive(FromEure, IntoEure)]` struct field
//! exactly where a `toml::Value` used to go (including behind `#[eure(flatten)]`
//! and inside `HashMap<String, EureValue>`/`Vec<EureValue>`).

use eure::document::constructor::DocumentConstructor;
use eure::document::node::Node;
use eure::document::parse::{ParseContext, ParseError};
use eure::document::path::PathSegment;
use eure::document::value::{ObjectKey, PrimitiveValue};
use eure::document::write::WriteError;
use eure::document::{EureDocument, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub struct EureValue(EureDocument);

impl EureValue {
    #[must_use]
    pub fn from_document(doc: EureDocument) -> Self {
        Self(doc)
    }

    #[must_use]
    pub fn document(&self) -> &EureDocument {
        &self.0
    }

    #[must_use]
    pub fn into_document(self) -> EureDocument {
        self.0
    }

    fn root(&self) -> &Node {
        self.0.node(self.0.get_root_id())
    }

    fn child(&self, id: NodeId) -> Self {
        Self(self.0.node_subtree_to_document(id))
    }

    #[must_use]
    pub fn is_table(&self) -> bool {
        self.root().as_map().is_some()
    }

    #[must_use]
    pub fn is_array(&self) -> bool {
        self.root().as_array().is_some()
    }

    /// A short label for this value's shape, for error messages (mirrors
    /// `toml::Value::type_str`).
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self.0.node(self.0.get_root_id()).content {
            eure::document::node::NodeValue::Hole(_) => "hole",
            eure::document::node::NodeValue::Primitive(PrimitiveValue::Null) => "null",
            eure::document::node::NodeValue::Primitive(PrimitiveValue::Bool(_)) => "boolean",
            eure::document::node::NodeValue::Primitive(PrimitiveValue::Integer(_)) => "integer",
            eure::document::node::NodeValue::Primitive(
                PrimitiveValue::F32(_) | PrimitiveValue::F64(_),
            ) => "float",
            eure::document::node::NodeValue::Primitive(PrimitiveValue::Text(_)) => "string",
            eure::document::node::NodeValue::Array(_) => "array",
            eure::document::node::NodeValue::Tuple(_) => "tuple",
            eure::document::node::NodeValue::Map(_) | eure::document::node::NodeValue::PartialMap(_) => {
                "table"
            }
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        self.root().as_primitive()?.as_str()
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self.root().as_primitive()? {
            PrimitiveValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as an integer. Eure integers are arbitrary precision; this
    /// truncates to `i64`, which is every integer this engine ever writes.
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self.root().as_primitive()? {
            PrimitiveValue::Integer(i) => i.to_string().parse().ok(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self.root().as_primitive()? {
            PrimitiveValue::F64(f) => Some(*f),
            PrimitiveValue::F32(f) => Some(f64::from(*f)),
            PrimitiveValue::Integer(i) => i.to_string().parse().ok(),
            _ => None,
        }
    }

    /// The field named `key`, if this value is a table and has one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Self> {
        let id = self.root().as_map()?.get_node_id(&ObjectKey::String(key.to_string()))?;
        Some(self.child(id))
    }

    /// This value's fields in document order, if it is a table.
    pub fn iter_table(&self) -> Box<dyn Iterator<Item = (String, Self)> + '_> {
        match self.root().as_map() {
            Some(map) => Box::new(
                map.iter()
                    .map(|(key, &id)| (object_key_to_string(key), self.child(id))),
            ),
            None => Box::new(std::iter::empty()),
        }
    }

    /// This value's items in order, if it is an array.
    #[must_use]
    pub fn as_array_items(&self) -> Vec<Self> {
        self.root()
            .as_array()
            .map(|array| array.to_vec().into_iter().map(|id| self.child(id)).collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn null() -> Self {
        Self(EureDocument::new_primitive(PrimitiveValue::Null))
    }

    #[must_use]
    pub fn string(s: impl Into<String>) -> Self {
        Self(EureDocument::new_primitive(PrimitiveValue::from(s.into())))
    }

    #[must_use]
    pub fn bool(b: bool) -> Self {
        Self(EureDocument::new_primitive(PrimitiveValue::from(b)))
    }

    #[must_use]
    pub fn integer(i: i64) -> Self {
        Self(EureDocument::new_primitive(PrimitiveValue::from(i)))
    }

    #[must_use]
    pub fn float(f: f64) -> Self {
        Self(EureDocument::new_primitive(PrimitiveValue::from(f)))
    }

    /// Builds a table from `entries`, in the order given.
    #[must_use]
    pub fn table(entries: impl IntoIterator<Item = (String, Self)>) -> Self {
        let mut c = DocumentConstructor::new();
        c.bind_empty_map().expect("a fresh document root can always become a map");
        for (key, value) in entries {
            let scope = c.begin_scope();
            c.navigate(PathSegment::Value(ObjectKey::String(key)))
                .expect("inserting into a just-created map cannot fail");
            c.write_subtree(&value.0, value.0.get_root_id())
                .expect("splicing a self-contained subtree cannot fail");
            c.end_scope(scope).expect("the scope just opened is still open");
        }
        Self(c.finish())
    }

    /// Builds an array from `items`, in order.
    #[must_use]
    pub fn array(items: impl IntoIterator<Item = Self>) -> Self {
        let mut c = DocumentConstructor::new();
        c.bind_empty_array().expect("a fresh document root can always become an array");
        for item in items {
            let scope = c.begin_scope();
            c.navigate(PathSegment::ArrayIndex(
                eure::document::path::ArrayIndexKind::Push,
            ))
                .expect("pushing onto a just-created array cannot fail");
            c.write_subtree(&item.0, item.0.get_root_id())
                .expect("splicing a self-contained subtree cannot fail");
            c.end_scope(scope).expect("the scope just opened is still open");
        }
        Self(c.finish())
    }
}

impl EureValue {
    /// Parses this value as a concrete type, the same way a document parses
    /// into a `#[derive(FromEure)]` struct.
    pub fn parse<'doc, T: eure::FromEure<'doc, T>>(&'doc self) -> Result<T, T::Error> {
        self.0.parse(self.0.get_root_id())
    }
}

impl Default for EureValue {
    fn default() -> Self {
        Self::table([])
    }
}

fn object_key_to_string(key: &ObjectKey) -> String {
    match key {
        ObjectKey::String(s) => s.clone(),
        ObjectKey::Number(_) | ObjectKey::Tuple(_) => key.to_string(),
    }
}

impl<'doc> eure::FromEure<'doc> for EureValue {
    type Error = ParseError;

    fn parse(ctx: &ParseContext<'doc>) -> Result<Self, Self::Error> {
        <EureDocument as eure::FromEure>::parse(ctx).map(Self)
    }
}

impl eure::IntoEure for EureValue {
    type Error = WriteError;

    fn write(value: Self, c: &mut DocumentConstructor) -> Result<(), Self::Error> {
        <EureDocument as eure::IntoEure>::write(value.0, c)
    }
}
