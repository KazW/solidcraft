use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    BreakLinkParams, BreakLinkResult, LinkDesignParams, LinkDesignResult, LinksParams, LinksResult, RelinkParams, RelinkResult, UpdateLinksParams,
    UpdateLinksResult,
};
use crate::Session;
use crate::schema::tests::{assert_keys, assert_strict_keeps_todays_errors, conforms};
use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<LinkDesignParams>(), &["path", "scope"]);
    assert_keys(input_schema_for::<UpdateLinksParams>(), &["design"]);
    assert_keys(input_schema_for::<RelinkParams>(), &["design", "path"]);
    assert_keys(input_schema_for::<BreakLinkParams>(), &["design"]);
    assert_keys(input_schema_for::<LinksParams>(), &[]);
}

#[test]
fn strict_keeps_todays_errors() {
    assert_strict_keeps_todays_errors(&[
        ("parameters.link_design", json!({})),
        ("parameters.link_design", json!({"path": ""})),
        ("parameters.link_design", json!({"path": "  "})),
        ("parameters.link_design", json!({"path": 5})),
        ("parameters.link_design", json!({"path": null})),
        ("parameters.link_design", json!({"path": "x".repeat(4096)})),
        ("parameters.update_links", json!({})),
        ("parameters.relink", json!({})),
        ("parameters.break_link", json!({})),
    ]);
}

/// The contracts say what the handlers need: `path` is required where the handler refuses
/// without it, though the type leaves it to the handler's message.
#[test]
fn the_schemas_take_what_the_handlers_read() {
    let takes = |schema: schemars::Schema, cases: &[(Value, bool)]| {
        let schema = schema.as_value();
        for (p, takes) in cases {
            assert_eq!(conforms(schema, schema, p), *takes, "{p}");
        }
    };
    takes(
        input_schema_for::<LinkDesignParams>(),
        &[
            (json!({"path": "standard.sc"}), true),
            (json!({"path": "standard.sc", "scope": "favorites"}), true),
            (json!({}), false),
            (json!({"path": ""}), false),
            (json!({"path": "standard.sc", "extra": 1}), false),
        ],
    );
    takes(input_schema_for::<UpdateLinksParams>(), &[(json!({}), true), (json!({"design": "standard"}), true), (json!({"design": 3}), false)]);
    takes(
        input_schema_for::<RelinkParams>(),
        &[(json!({"path": "b.sc"}), true), (json!({"design": "a", "path": "b.sc"}), true), (json!({"design": "a"}), false), (json!({}), false)],
    );
    takes(input_schema_for::<BreakLinkParams>(), &[(json!({}), true), (json!({"design": "a"}), true), (json!({"design": []}), false)]);
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-parameter-link-schema-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    dir
}

/// A design file with one user parameter, `clearance`, of `value` mm.
fn standard(path: &Path, value: f64) {
    let mut part = Session::default();
    for (id, p) in [("parameters.add", json!({"name": "clearance", "expression": value, "unit": "mm"})), ("file.save", json!({"path": path}))] {
        part.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

fn matches(schema: schemars::Schema, id: &str, reply: &Value) {
    let schema = schema.as_value();
    assert!(conforms(schema, schema, reply), "{id}: {reply} doesn't match {schema}");
    assert!(reply.get("param_warnings").is_none(), "{id}: {reply}");
}

/// Link, list, update, relink and break, in Warn: each real reply matches its output schema and
/// carries no warning.
#[test]
fn link_replies_match_the_output_schema() {
    let dir = folder("links");
    let (a, b) = (dir.join("a.sc"), dir.join("b.sc"));
    standard(&a, 0.2);
    standard(&b, 0.3);
    let mut s = Session { param_check: ParamCheck::Warn, ..Session::default() };
    let mut run = |id: &str, p: Value| s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"));
    let linked = run("parameters.link_design", json!({"path": a}));
    assert_eq!(linked["parameters"], json!(["clearance"]), "{linked}");
    assert_eq!(linked["scope"], json!("all"), "{linked}");
    matches(output_schema_for::<LinkDesignResult>(), "parameters.link_design", &linked);

    let listed = run("parameters.links", json!({}));
    assert_eq!(listed["designs"][0]["state"], json!("up_to_date"), "{listed}");
    matches(output_schema_for::<LinksResult>(), "parameters.links", &listed);

    // The file changed: the list says which parameter, and the update brings it in.
    standard(&a, 0.25);
    let stale = run("parameters.links", json!({}));
    assert_eq!(stale["designs"][0]["state"], json!("out_of_date"), "{stale}");
    assert_eq!(stale["designs"][0]["changed"], json!(["clearance"]), "{stale}");
    matches(output_schema_for::<LinksResult>(), "parameters.links", &stale);
    let update = run("parameters.update_links", json!({}));
    assert_eq!(update["updated"][0]["changed"][0]["to"], json!("0.25 mm"), "{update}");
    matches(output_schema_for::<UpdateLinksResult>(), "parameters.update_links", &update);
    let nothing = run("parameters.update_links", json!({}));
    assert_eq!(nothing["updated"], json!([]), "{nothing}");
    assert_eq!(nothing["unchanged"].as_array().map(Vec::len), Some(1), "{nothing}");
    matches(output_schema_for::<UpdateLinksResult>(), "parameters.update_links", &nothing);

    let moved = run("parameters.relink", json!({"path": b}));
    assert_eq!(moved["changed"][0]["to"], json!("0.3 mm"), "{moved}");
    matches(output_schema_for::<RelinkResult>(), "parameters.relink", &moved);
    let broken = run("parameters.break_link", json!({}));
    assert_eq!(broken["parameters"], json!(["clearance"]), "{broken}");
    matches(output_schema_for::<BreakLinkResult>(), "parameters.break_link", &broken);
    assert_eq!(run("parameters.links", json!({}))["designs"], json!([]));

    // A misspelt key still runs in Warn, and says so, with a reply that matches.
    let warned = run("parameters.links", json!({"lnks": true}));
    assert!(warned["param_warnings"].as_array().is_some_and(|w| !w.is_empty()), "{warned}");
    let schema = output_schema_for::<LinksResult>();
    assert!(conforms(schema.as_value(), schema.as_value(), &warned), "{warned}");
    let _ = std::fs::remove_dir_all(&dir);
}
