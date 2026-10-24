//! Linked designs (Link Parameters): a design uses the user parameters of other design files by
//! name. A linked design is read and only its parameter table evaluated; the values come in
//! read-only and are kept in this design, so it opens and evaluates without the file. What a new
//! set of values may change is decided in `solidcraft_doc::linked`; this reads the files.
//!
//! In memory each linked design keeps its file as resolved; a design file keeps it relative to
//! its own folder ([`file_form`] when writing, [`opened`] when reading), so a folder of designs
//! can move or sync between computers, and Save As, saving on a copy and undo need no rewriting.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solidcraft_doc::{Document, LinkedDesign, LinkedParam, ParamScope, expr};

use super::CommandSpec;
use super::design_link::{LinkProblem, LinkState, absolute, link_presence, read_linked_file, rebase, resolve, same_file};
use super::parameters::failing;
use crate::params::{bad, str_};
use crate::schema::parameters_link::{
    BreakLinkParams, BreakLinkResult, LinkChange, LinkDesignParams, LinkDesignResult, LinkedDesignInfo, LinksParams, LinksResult, RelinkParams,
    RelinkResult, UpdateLinksParams, UpdateLinksResult, UpdatedLink,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

#[cfg(test)]
#[path = "parameters_link_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

const LINK_DESIGN_CONTRACT: TypedContract<LinkDesignParams> = TypedContract::new();
const UPDATE_LINKS_CONTRACT: TypedContract<UpdateLinksParams> = TypedContract::new();
const RELINK_CONTRACT: TypedContract<RelinkParams> = TypedContract::new();
const BREAK_LINK_CONTRACT: TypedContract<BreakLinkParams> = TypedContract::new();
const LINKS_CONTRACT: TypedContract<LinksParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("parameters.link_design", "Link Parameters", crate::checked!("parameters.link_design", link_design))
        .icon("chain")
        .contract(&LINK_DESIGN_CONTRACT)
        .output(output_schema_for::<LinkDesignResult>)
        .params(
            "path: a .sc design → its user parameters come in read-only and can be used by name in this design; \
         scope?: all|favorites (default all) → design (its path as this design's file keeps it), file, scope, parameters, recomputed, errors",
        ),
    CommandSpec::new("parameters.update_links", "Update Parameter Links", crate::checked!("parameters.update_links", update_links))
        .icon("update")
        .contract(&UPDATE_LINKS_CONTRACT)
        .output(output_schema_for::<UpdateLinksResult>)
        .params(
            "design?: a linked design's path or file name (default: every linked design) — reads the linked designs again, all or nothing → \
         updated [{design, changed [{name, from, to}], added, removed}], unchanged, recomputed, errors",
        ),
    CommandSpec::new("parameters.relink", "Relink Parameters", crate::checked!("parameters.relink", relink))
        .icon("update")
        .contract(&RELINK_CONTRACT)
        .output(output_schema_for::<RelinkResult>)
        .params(
            "design?: a linked design's path or file name (needed when there are several); path: the .sc design to read instead \
         (moved or renamed) — read before anything changes → design, file, previous, changed, added, removed, recomputed, errors",
        ),
    CommandSpec::new("parameters.break_link", "Break Parameter Link", crate::checked!("parameters.break_link", break_link))
        .icon("break")
        .contract(&BREAK_LINK_CONTRACT)
        .output(output_schema_for::<BreakLinkResult>)
        .params(
            "design?: a linked design's path or file name (needed when there are several) — its parameters become ordinary user \
         parameters with the values last read, and the file is forgotten → design, parameters",
        ),
    CommandSpec::new("parameters.links", "List Parameter Links", crate::checked!("parameters.links", list_links))
        .icon("chain")
        .noundo()
        .contract(&LINKS_CONTRACT)
        .output(output_schema_for::<LinksResult>)
        .params(
            "→ designs: design (as this design's file keeps it), file, scope, parameters, \
         state: up_to_date|out_of_date|missing|unreadable|not_allowed (outside the folders this agent may use), \
         changed (names whose value differs in the file), error?",
        ),
];

/// How far, and through how many files, a loop of parameter links is looked for.
const MAX_DEPTH: usize = 16;
const MAX_VISITS: usize = 256;

/// The design at `file`, linked from the design saved at `own`, read as it is saved. A design a
/// newer SolidCraft saved opens like any other (its parameters are all that is used here).
fn read_design(file: &Path, own: Option<&str>) -> std::result::Result<Document, LinkProblem> {
    let bytes = read_linked_file(file, own)?;
    solidcraft_io::read_design(&bytes).map_err(|e| LinkProblem { state: LinkState::Unreadable, reason: format!("{}: {e}", file.display()) })
}

/// A value as a number in `unit` with the unit written in (`8.2 mm`), rounded to 10 decimals so
/// a round trip through another unit doesn't show as a change.
fn literal(v: expr::Value, unit: &str) -> Option<String> {
    let x = expr::value_in_unit(v, unit);
    if !x.is_finite() {
        return None;
    }
    let mut n = format!("{x:.10}");
    if n.contains('.') {
        n = n.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if n == "-0" {
        n = "0".into();
    }
    let unit = unit.trim();
    Some(if unit.is_empty() { n } else { format!("{n} {unit}") })
}

/// The user parameters `doc` shares under `scope`, as values in their own units.
fn shared(doc: &Document, scope: ParamScope, shown: &str) -> Result<Vec<LinkedParam>> {
    let (vals, errs) = doc.param_values();
    let mut out: Vec<LinkedParam> = Vec::new();
    for p in doc.params.iter().filter(|p| !p.model) {
        if scope == ParamScope::Favorites && !doc.favorites.contains(&p.name) {
            continue;
        }
        if let Some(e) = errs.get(&p.name) {
            return Err(EngineError::Other(format!("{shown}: parameter `{}` has an error ({e}); fix it there first", p.name)));
        }
        let Some(e) = vals.get(&p.name).and_then(|v| literal(*v, &p.unit)) else {
            return Err(EngineError::Other(format!("{shown}: parameter `{}` has no value", p.name)));
        };
        out.push(LinkedParam { name: p.name.clone(), expr: e, unit: p.unit.trim().to_string(), comment: p.comment.clone() });
    }
    if out.is_empty() {
        return Err(EngineError::Other(match scope {
            ParamScope::Favorites => format!("{shown}: the design has no favourite user parameters (link it with scope `all`)"),
            ParamScope::All => format!("{shown}: the design has no user parameters"),
        }));
    }
    Ok(out)
}

/// Whether `doc` (saved at `file`) uses the parameters of the design at `own` through its own
/// parameter links, directly or further along. A file that can't be read can't be checked; it
/// doesn't refuse the link.
fn links_back(doc: &Document, file: &Path, own: &Path) -> bool {
    let at = file.to_string_lossy().into_owned();
    let mut stack: Vec<(PathBuf, String, usize)> = doc.linked_designs.iter().map(|x| (resolve(Some(&at), &x.path), at.clone(), 1)).collect();
    let mut seen = BTreeSet::new();
    while let Some((f, from, depth)) = stack.pop() {
        if same_file(&f, own) {
            return true;
        }
        if depth >= MAX_DEPTH || seen.len() >= MAX_VISITS || !seen.insert(absolute(&f)) {
            continue;
        }
        let Ok(next) = read_design(&f, Some(&from)) else { continue };
        let at = f.to_string_lossy().into_owned();
        stack.extend(next.linked_designs.iter().map(|x| (resolve(Some(&at), &x.path), at.clone(), depth + 1)));
    }
    false
}

/// The design at `given`, read and checked as a linked design of `s`: a design file, not this
/// design itself, not already linked (other than as `except`), and not using this design's
/// parameters in turn. Returns its file and its parameters under `scope`.
fn read_linked(s: &Session, given: &str, scope: ParamScope, except: Option<usize>, cmd: &str) -> Result<(PathBuf, Vec<LinkedParam>)> {
    let file = absolute(Path::new(given));
    let shown = file.display().to_string();
    let own = s.path.as_deref();
    if own.is_some_and(|o| same_file(Path::new(o), &file)) {
        return Err(bad(cmd, "a design can't link its own parameters"));
    }
    if s.doc.linked_designs.iter().enumerate().any(|(i, x)| Some(i) != except && same_file(Path::new(&x.path), &file)) {
        return Err(bad(cmd, "this design already links that design's parameters"));
    }
    let doc = read_design(&file, own).map_err(|e| bad(cmd, e.to_string()))?;
    if own.is_some_and(|o| links_back(&doc, &file, Path::new(o))) {
        return Err(bad(cmd, format!("{shown} uses this design's parameters through its own links, so linking it would make a loop")));
    }
    let params = shared(&doc, scope, &shown).map_err(|e| bad(cmd, e.to_string()))?;
    Ok((file, params))
}

fn scope_arg(p: &Value, cmd: &str) -> Result<ParamScope> {
    match str_(p, "scope").map(str::to_ascii_lowercase).as_deref() {
        None | Some("all") => Ok(ParamScope::All),
        Some("favorites" | "favourites" | "favorite" | "favourite") => Ok(ParamScope::Favorites),
        Some(other) => Err(bad(cmd, format!("unknown scope `{other}` (all or favorites)"))),
    }
}

/// How the design file of `s` keeps a linked design's path: relative to its folder once saved.
fn kept(s: &Session, path: &str) -> String {
    match s.path.as_deref() {
        Some(own) => rebase(path, None, own),
        None => path.to_string(),
    }
}

/// The linked design `design` names: its file, its path as this design's file keeps it, or the
/// file's name (with or without `.solidcraft`). Without `design`, the only one there is.
fn design_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let designs = &s.doc.linked_designs;
    let Some(key) = str_(p, "design").map(str::trim).filter(|k| !k.is_empty()) else {
        return match (designs.len(), p.get("design")) {
            (0, _) => Err(bad(cmd, "this design links no parameters")),
            (1, None) => Ok(0),
            _ => Err(bad(cmd, "`design` is required: give a linked design's path or file name")),
        };
    };
    let file = absolute(Path::new(key));
    let hits: Vec<usize> = designs
        .iter()
        .enumerate()
        .filter(|(_, x)| {
            let f = Path::new(&x.path);
            x.path == key
                || kept(s, &x.path) == key
                || absolute(f) == file
                || [f.file_name(), f.file_stem()].iter().flatten().any(|n| n.to_string_lossy() == key)
        })
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => Err(bad(cmd, format!("no linked design `{key}`"))),
        _ => Err(bad(cmd, format!("several linked designs match `{key}`; give its path"))),
    }
}

/// A command's reply as JSON: these commands build their replies from the types their output
/// schemas describe, so the two can't drift apart.
fn reply(cmd: &str, reply: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(reply).map_err(|e| bad(cmd, e.to_string()))
}

fn link_design(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.link_design";
    let given = str_(p, "path").filter(|x| !x.trim().is_empty() && x.len() < 4096).ok_or_else(|| bad(cmd, "`path` is required"))?;
    let scope = scope_arg(p, cmd)?;
    let (file, params) = read_linked(s, given, scope, None, cmd)?;
    let path = file.to_string_lossy().into_owned();
    let solved = s.solved_sketches();
    let change = s.doc_mut().set_linked(None, LinkedDesign { path: path.clone(), scope, params })?;
    s.settle_value_edit(&solved)?;
    reply(
        cmd,
        LinkDesignResult {
            design: kept(s, &path),
            file: path,
            scope: scope.into(),
            parameters: change.added,
            recomputed: s.model.last_recomputed,
            errors: failing(s),
            param_warnings: Vec::new(),
        },
    )
}

fn update_links(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.update_links";
    let which: Vec<usize> = if p.get("design").is_some() {
        vec![design_index(s, p, cmd)?]
    } else if s.doc.linked_designs.is_empty() {
        return Err(bad(cmd, "this design links no parameters"));
    } else {
        (0..s.doc.linked_designs.len()).collect()
    };
    let own = s.path.clone();
    // Every linked design is read and checked before anything changes: all or nothing.
    let mut next = (*s.doc).clone();
    let (mut updated, mut unchanged) = (Vec::new(), Vec::new());
    for i in which {
        let Some(design) = next.linked_designs.get(i).cloned() else { continue };
        let file = PathBuf::from(&design.path);
        let shown = file.display().to_string();
        let doc = read_design(&file, own.as_deref()).map_err(|e| bad(cmd, e.to_string()))?;
        if own.as_deref().is_some_and(|o| links_back(&doc, &file, Path::new(o))) {
            return Err(bad(cmd, format!("{shown} now uses this design's parameters through its own links, which makes a loop")));
        }
        let params = shared(&doc, design.scope, &shown).map_err(|e| bad(cmd, e.to_string()))?;
        if params == design.params {
            unchanged.push(kept(s, &design.path));
            continue;
        }
        let change = next.set_linked(Some(i), LinkedDesign { params, ..design.clone() }).map_err(|e| bad(cmd, format!("{shown}: {e}")))?;
        updated.push(UpdatedLink { design: kept(s, &design.path), change: LinkChange::from(&change) });
    }
    if !updated.is_empty() {
        let solved = s.solved_sketches();
        *s.doc_mut() = next;
        s.settle_value_edit(&solved)?;
    }
    reply(cmd, UpdateLinksResult { updated, unchanged, recomputed: s.model.last_recomputed, errors: failing(s), param_warnings: Vec::new() })
}

fn relink(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.relink";
    let i = design_index(s, p, cmd)?;
    let given = str_(p, "path").filter(|x| !x.trim().is_empty() && x.len() < 4096).ok_or_else(|| bad(cmd, "`path` is required"))?;
    let design = s.doc.linked_designs.get(i).cloned().ok_or_else(|| bad(cmd, "no such linked design"))?;
    let (file, params) = read_linked(s, given, design.scope, Some(i), cmd)?;
    let path = file.to_string_lossy().into_owned();
    let solved = s.solved_sketches();
    let change = s.doc_mut().set_linked(Some(i), LinkedDesign { path: path.clone(), scope: design.scope, params })?;
    s.settle_value_edit(&solved)?;
    reply(
        cmd,
        RelinkResult {
            design: kept(s, &path),
            file: path,
            previous: kept(s, &design.path),
            change: LinkChange::from(&change),
            recomputed: s.model.last_recomputed,
            errors: failing(s),
            param_warnings: Vec::new(),
        },
    )
}

fn break_link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parameters.break_link";
    let i = design_index(s, p, cmd)?;
    let path = s.doc.linked_designs.get(i).map(|x| x.path.clone()).unwrap_or_default();
    let solved = s.solved_sketches();
    let names = s.doc_mut().break_linked(i)?;
    s.settle_value_edit(&solved)?;
    reply(cmd, BreakLinkResult { design: kept(s, &path), parameters: names, param_warnings: Vec::new() })
}

/// Where a linked design stands against its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedState {
    /// The linked design's file (as resolved).
    pub file: String,
    /// Where the link stands: `up_to_date`, `out_of_date`, `missing`, `unreadable` or
    /// `not_allowed` (outside the folders this agent may use).
    pub state: LinkState,
    /// Parameters whose value differs in the file, or that it has and this design hasn't yet.
    pub changed: Vec<String>,
    pub error: Option<String>,
}

impl LinkedState {
    /// Anything but up to date: the values in use are not the file's.
    pub fn needs_attention(&self) -> bool {
        self.state != LinkState::UpToDate
    }
}

/// Where each linked design stands against its file, for the design saved at `own`. Reading the
/// files to compare values is the costly part, so it happens when someone asks
/// (`parameters.links`, the desktop window's watcher on its own thread), never on open. It needs
/// no session, so it can run anywhere.
pub fn linked_design_states(designs: &[LinkedDesign], own: Option<&str>) -> Vec<LinkedState> {
    designs.iter().map(|design| linked_state(design, own)).collect()
}

fn linked_state(design: &LinkedDesign, own: Option<&str>) -> LinkedState {
    let file = Path::new(&design.path);
    let out = |state, changed, error| LinkedState { file: design.path.clone(), state, changed, error };
    let read = match read_design(file, own) {
        Ok(read) => read,
        Err(why) => return out(why.state, Vec::new(), Some(why.reason)),
    };
    match shared(&read, design.scope, &design.path) {
        Ok(now) => {
            let changed: Vec<String> = design
                .params
                .iter()
                .filter(|o| now.iter().find(|x| x.name == o.name).is_none_or(|x| x.expr != o.expr))
                .map(|o| o.name.clone())
                .chain(now.iter().filter(|x| !design.params.iter().any(|o| o.name == x.name)).map(|x| x.name.clone()))
                .collect();
            out(if changed.is_empty() { LinkState::UpToDate } else { LinkState::OutOfDate }, changed, None)
        }
        Err(e) => out(LinkState::Unreadable, Vec::new(), Some(e.to_string())),
    }
}

fn list_links(s: &mut Session, _p: &Value) -> Result<Value> {
    let designs: Vec<LinkedDesignInfo> = s
        .doc
        .linked_designs
        .iter()
        .zip(linked_design_states(&s.doc.linked_designs, s.path.as_deref()))
        .map(|(d, st)| LinkedDesignInfo {
            design: kept(s, &d.path),
            file: st.file,
            scope: d.scope.into(),
            parameters: d.params.len(),
            state: st.state,
            changed: st.changed,
            error: st.error,
        })
        .collect();
    reply("parameters.links", LinksResult { designs, param_warnings: Vec::new() })
}

/// What to tell someone opening the design: linked designs whose file can't be found or read,
/// and names the file holds twice (from a merge or a hand edit; see `linked_clashes`). Only the
/// files' presence is checked here; whether their values changed is worked out when asked
/// (`parameters.links`).
pub(super) fn open_warnings(s: &Session) -> Vec<String> {
    let own = s.path.as_deref();
    let missing = s.doc.linked_designs.iter().filter_map(|x| {
        link_presence(Path::new(&x.path), own).map(|why| format!("linked parameters: {}; the values last read are used", why.reason))
    });
    missing.chain(s.doc.linked_clashes().into_iter().map(|c| format!("linked parameters: {c}"))).collect()
}

/// The linked designs of `doc` as a design file written at `to` keeps them (`from`: where the
/// design was saved before): relative to its folder. Before the design has a file, `to` is
/// `None` and the paths stay whole.
pub(super) fn file_form(doc: &Document, from: Option<&str>, to: Option<&str>) -> Vec<LinkedDesign> {
    let kept = |path: &str| to.map_or_else(|| path.to_string(), |to| rebase(path, from, to));
    doc.linked_designs.iter().map(|x| LinkedDesign { path: kept(&x.path), ..x.clone() }).collect()
}

/// A design just read from `file`: its linked designs' paths resolved against that file.
pub(super) fn opened(doc: &mut Document, file: &str) {
    for x in &mut doc.linked_designs {
        x.path = absolute(&resolve(Some(file), &x.path)).to_string_lossy().into_owned();
    }
}
