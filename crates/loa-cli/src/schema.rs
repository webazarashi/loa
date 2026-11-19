//! `loa schema [command]` — self-description for scripts and agents (spec 3-1).

use crate::cli::Cli;
use clap::CommandFactory;
use loa_core::error::{LoaError, Result};
use loa_core::output::Envelope;
use serde_json::{Map, Value, json};

pub fn run(target: &[String]) -> Result<Envelope> {
    let joined = target.join(" ");
    let schema = match joined.as_str() {
        "" => return Ok(index()),
        "loa.yaml" | "spec" => to_value(schemars::schema_for!(
            loa_core::actions::declarative::SpecFile
        )),
        "scenario" => to_value(schemars::schema_for!(loa_core::scenario::Scenario)),
        "output" => to_value(schemars::schema_for!(Envelope)),
        "error" => to_value(schemars::schema_for!(LoaError)),
        "task" => to_value(schemars::schema_for!(loa_core::task::TaskStatus)),
        path => command_schema(path)?,
    };
    Ok(Envelope::new("schema", "got", schema))
}

fn to_value<T: serde::Serialize>(s: T) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

fn index() -> Envelope {
    let mut cmds = Vec::new();
    fn walk(c: &clap::Command, prefix: &str, out: &mut Vec<Value>) {
        for sub in c.get_subcommands() {
            if sub.is_hide_set() {
                continue;
            }
            let path = if prefix.is_empty() {
                sub.get_name().to_string()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if sub.has_subcommands() {
                walk(sub, &path, out);
            } else {
                out.push(json!({"command": path, "about": sub.get_about().map(|s| s.to_string())}));
            }
        }
    }
    walk(&Cli::command(), "", &mut cmds);
    Envelope::new(
        "schema",
        "listed",
        json!({
            "commands": cmds,
            "documents": ["loa.yaml", "scenario", "output", "error", "task"],
            "exit_codes": loa_core::error::Code::ALL.iter().map(|c| json!({"code": c.as_str(), "exit": c.exit_code(), "hint": c.default_hint()})).collect::<Vec<_>>(),
        }),
    )
}

/// JSON Schema of a command's arguments, built from the clap definition.
fn command_schema(path: &str) -> Result<Value> {
    let root = Cli::command();
    let mut cur = &root;
    for word in path.split_whitespace() {
        cur = cur
            .get_subcommands()
            .find(|s| s.get_name() == word && !s.is_hide_set())
            .ok_or_else(|| {
                LoaError::usage(format!("コマンド {path} はありません")).with_next("loa schema")
            })?;
    }
    if cur.has_subcommands() {
        let subs: Vec<String> = cur
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .map(|s| format!("{path} {}", s.get_name()))
            .collect();
        return Ok(json!({"command": path, "subcommands": subs}));
    }
    let mut props = Map::new();
    let mut required = Vec::new();
    let globals: Vec<clap::Arg> = root.get_arguments().cloned().collect();
    for a in cur.get_arguments().chain(globals.iter()) {
        let id = a.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        let multiple = matches!(a.get_action(), clap::ArgAction::Append);
        let flag = matches!(a.get_action(), clap::ArgAction::SetTrue);
        let ty = if flag {
            json!("boolean")
        } else if multiple {
            json!("array")
        } else {
            json!("string")
        };
        let mut p = Map::new();
        p.insert("type".into(), ty);
        if multiple {
            p.insert("items".into(), json!({"type": "string"}));
        }
        if let Some(h) = a.get_help() {
            p.insert("description".into(), json!(h.to_string()));
        }
        let name = match a.get_long() {
            Some(l) => format!("--{l}"),
            None => id.to_string(),
        };
        p.insert("cli".into(), json!(name));
        if let Some(d) = a.get_default_values().first() {
            p.insert("default".into(), json!(d.to_string_lossy()));
        }
        if a.is_required_set() {
            required.push(json!(id));
        }
        props.insert(id.to_string(), Value::Object(p));
    }
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": format!("loa {path}"),
        "description": cur.get_about().map(|s| s.to_string()),
        "type": "object",
        "properties": props,
        "required": required,
        "output": {"$ref": "loa schema output"},
        "error": {"$ref": "loa schema error"},
    }))
}
