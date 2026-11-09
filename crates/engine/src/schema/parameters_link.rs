//! Contracts and result types for the linked-parameter commands in `cmd/parameters_link.rs`:
//! `parameters.link_design`, `parameters.update_links`, `parameters.relink`,
//! `parameters.break_link` and `parameters.links` (#5354).
//!
//! The handlers read their parameters from the raw JSON, so each key here reads as its handler
//! reads it and fails with the handler's message. Whether a design file or a linked design
//! exists stays the handler's to check. A key the handler refuses when it is left out (a path)
//! is optional in the type and required in the schema, so that the handler's message, not the
//! check's "is required", is what a caller sees.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solidcraft_doc::ParamScope;
use solidcraft_doc::linked::{LinkedChange, ValueChange};

use super::component_link::{path_required, path_schema, require};
use super::parameters::FailedFeature;
use crate::cmd::LinkState;

/// `path` as the handlers read it, required in the schema and left to the handler's message in
/// the type.
fn path_only_required(schema: &mut Schema) {
    require(schema, "path");
    // The type's `default` for an absent `scope` is null, which the text doesn't take.
    if let Some(Value::Object(scope)) = schema.get_mut("properties").and_then(|p| p.get_mut("scope")) {
        scope.insert("default".into(), "all".into());
    }
}

/// The `scope` text as the handler reads it: any case, with the US and UK spellings of
/// favourites. Left to the handler to refuse an unknown word.
fn scope_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "Which of the design's user parameters come in, ignoring case: `all` or `favorites` (also spelt `favourites`, `favorite`, `favourite`). Default: all.",
        "type": "string"
    })
}

/// `path` as the shared schema has it, and at least one character: the handler refuses an
/// empty path.
fn link_path_schema(g: &mut SchemaGenerator) -> Schema {
    let mut schema = path_schema(g);
    schema.insert("minLength".into(), 1.into());
    schema
}

/// Which of a linked design's user parameters come in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkScope {
    /// All its user parameters.
    All,
    /// Its favourite user parameters.
    Favorites,
}

impl From<ParamScope> for LinkScope {
    fn from(scope: ParamScope) -> Self {
        match scope {
            ParamScope::All => LinkScope::All,
            ParamScope::Favorites => LinkScope::Favorites,
        }
    }
}

/// Use the user parameters of another design file by name, read-only. The design is read and
/// checked first: it must be a design file other than this one, not linked already, and not use
/// this design's parameters in turn. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields, transform = path_only_required)]
pub struct LinkDesignParams {
    // Optional in the type and required in the schema (`path_only_required`).
    /// The design (`.sc`) whose user parameters come in, by file path.
    #[serde(default, deserialize_with = "path_required", skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "link_path_schema")]
    pub path: Option<String>,
    /// Which of its user parameters come in: `all` or `favorites`. Default: all.
    #[serde(default)]
    #[schemars(schema_with = "scope_schema")]
    pub scope: Option<String>,
}

/// What `parameters.link_design` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LinkDesignResult {
    /// The linked design as this design's file keeps its path (relative to the design's folder
    /// once it is saved).
    pub design: String,
    /// The file, as resolved.
    pub file: String,
    /// Which of its parameters came in.
    pub scope: LinkScope,
    /// The names that came in.
    pub parameters: Vec<String>,
    /// How many features were rebuilt.
    pub recomputed: usize,
    /// The features that failed to rebuild.
    pub errors: Vec<FailedFeature>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A parameter's value before and after an update, with the unit written in (`8.2 mm`).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ParamValueChange {
    /// The parameter's name.
    pub name: String,
    /// Its value before.
    pub from: String,
    /// Its value now.
    pub to: String,
}

impl From<&ValueChange> for ParamValueChange {
    fn from(c: &ValueChange) -> Self {
        ParamValueChange { name: c.name.clone(), from: c.from.clone(), to: c.to.clone() }
    }
}

/// What a linked design's new values changed in this design.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LinkChange {
    /// The parameters whose value changed.
    pub changed: Vec<ParamValueChange>,
    /// The parameters this design has now and did not have. A rename in the linked design reads
    /// as one removed and one added.
    pub added: Vec<String>,
    /// The parameters this design no longer has.
    pub removed: Vec<String>,
}

impl From<&LinkedChange> for LinkChange {
    fn from(c: &LinkedChange) -> Self {
        LinkChange { changed: c.changed.iter().map(ParamValueChange::from).collect(), added: c.added.clone(), removed: c.removed.clone() }
    }
}

/// Read linked designs again and use their current values, all or nothing: every design is read
/// and checked before anything changes. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct UpdateLinksParams {
    /// One linked design: its path as `parameters.links` reports it, its file, or the file's
    /// name with or without `.sc`. Default: every linked design.
    #[serde(default)]
    pub design: Option<String>,
}

/// A linked design an update changed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpdatedLink {
    /// The linked design, as this design's file keeps its path.
    pub design: String,
    #[serde(flatten)]
    pub change: LinkChange,
}

/// What `parameters.update_links` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct UpdateLinksResult {
    /// The linked designs whose values changed.
    pub updated: Vec<UpdatedLink>,
    /// The linked designs that were read and hold what this design already had.
    pub unchanged: Vec<String>,
    /// How many features were rebuilt.
    pub recomputed: usize,
    /// The features that failed to rebuild.
    pub errors: Vec<FailedFeature>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Point a linked design at another file (moved or renamed) and use that file's values. The new
/// file is read and checked before anything changes. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields, transform = path_only_required)]
pub struct RelinkParams {
    /// The linked design: its path as `parameters.links` reports it, its file, or the file's
    /// name with or without `.sc`. Needed when there are several.
    #[serde(default)]
    pub design: Option<String>,
    // Optional in the type and required in the schema (`path_only_required`).
    /// The design (`.sc`) to read instead, by file path.
    #[serde(default, deserialize_with = "path_required", skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "link_path_schema")]
    pub path: Option<String>,
}

/// What `parameters.relink` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelinkResult {
    /// The linked design now, as this design's file keeps its path.
    pub design: String,
    /// The file, as resolved.
    pub file: String,
    /// The path it had.
    pub previous: String,
    #[serde(flatten)]
    pub change: LinkChange,
    /// How many features were rebuilt.
    pub recomputed: usize,
    /// The features that failed to rebuild.
    pub errors: Vec<FailedFeature>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Keep a linked design's parameters as ordinary user parameters with the values last read, and
/// forget its file. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct BreakLinkParams {
    /// The linked design: its path as `parameters.links` reports it, its file, or the file's
    /// name with or without `.sc`. Needed when there are several.
    #[serde(default)]
    pub design: Option<String>,
}

/// What `parameters.break_link` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BreakLinkResult {
    /// The design that was linked, as this design's file kept its path.
    pub design: String,
    /// The parameters that are ordinary user parameters now.
    pub parameters: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `parameters.links` takes no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LinksParams {}

/// What `parameters.links` returns: where each linked design stands against its file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LinksResult {
    pub designs: Vec<LinkedDesignInfo>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// One linked design.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LinkedDesignInfo {
    /// The path as this design's file keeps it.
    pub design: String,
    /// The file, as resolved.
    pub file: String,
    /// Which of its parameters come in.
    pub scope: LinkScope,
    /// How many parameters it brings in.
    pub parameters: usize,
    /// `up_to_date`, `out_of_date`, `missing`, `unreadable`, or `not_allowed` (a network location
    /// that is not allowed, or outside the folders this agent may use).
    pub state: LinkState,
    /// The parameters whose value differs in the file, or that it has and this design has not.
    pub changed: Vec<String>,
    /// Why the file can't be read, when it can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
#[path = "parameters_link_tests.rs"]
mod tests;
