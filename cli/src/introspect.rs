//! `sessile schema`: the command index and each command's contract as JSON.
//! Names, arguments, flags and defaults are read from the clap parser itself,
//! so they cannot drift from what the binary accepts; effects and output
//! schemas come from the table below.

use std::any::TypeId;

use clap::{Arg, ArgAction};
use serde_json::{Map, Value, json};

use crate::error::{Result, SessileError};
use crate::export::INLINE_MAX_BYTES;
use crate::output::write_json;

pub const SCHEMA_VERSION: &str = "1";
const INTROSPECTION: &str = "schema";

/// What the parser cannot say about a command.
struct Contract {
    effects: &'static str,
    confirm: bool,
    output: Value,
    output_description: Option<String>,
    format_defaults: Option<Value>,
}

pub fn print(path: &[String]) -> Result<()> {
    let value = if path.is_empty() {
        index()
    } else {
        let name = path.join(" ");
        detail(&name).ok_or_else(|| {
            SessileError::usage(format!("unknown command path '{name}'"))
                .with_hint("Run `sessile schema` for the command index")
        })?
    };
    write_json(&value);
    Ok(())
}

pub fn index() -> Value {
    let root = crate::cli::command();
    let mut commands: Vec<Value> = subcommands(&root)
        .map(|sub| {
            let name = sub.get_name();
            json!({
                "name": name,
                "description": about(sub),
                "effects": contract(name).effects,
            })
        })
        .collect();
    commands.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({
        "schema_version": SCHEMA_VERSION,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "global_flags": root.get_arguments().map(descriptor).collect::<Vec<_>>(),
        "format_defaults": {"tty": "text", "non_tty": "json"},
        "exit_codes": {
            "0": "success",
            "1": "failure",
            "2": "usage error",
            "130": "interrupted by Ctrl-C"
        },
        "commands": commands,
    })
}

pub fn detail(name: &str) -> Option<Value> {
    let root = crate::cli::command();
    let sub = subcommands(&root).find(|s| s.get_name() == name)?;
    let contract = contract(name);
    let (args, flags): (Vec<&Arg>, Vec<&Arg>) =
        sub.get_arguments().partition(|a| a.is_positional());
    let mut out = Map::new();
    out.insert("name".into(), json!(name));
    let description = sub
        .get_long_about()
        .or(sub.get_about())
        .map(ToString::to_string);
    out.insert("description".into(), json!(description.unwrap_or_default()));
    out.insert(
        "args".into(),
        Value::Array(args.into_iter().map(descriptor).collect()),
    );
    out.insert(
        "flags".into(),
        Value::Array(flags.into_iter().map(descriptor).collect()),
    );
    out.insert("effects".into(), json!(contract.effects));
    out.insert("confirm".into(), json!(contract.confirm));
    out.insert("interactive".into(), json!(false));
    out.insert("output".into(), contract.output);
    if let Some(text) = contract.output_description {
        out.insert("output_description".into(), json!(text));
    }
    if let Some(defaults) = contract.format_defaults {
        out.insert("format_defaults".into(), defaults);
    }
    Some(Value::Object(out))
}

fn subcommands(root: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    root.get_subcommands()
        .filter(|s| s.get_name() != INTROSPECTION)
}

fn about(sub: &clap::Command) -> String {
    sub.get_about().map(ToString::to_string).unwrap_or_default()
}

/// An I1 input descriptor read from the parser's own definition.
fn descriptor(arg: &Arg) -> Value {
    let is_bool = matches!(arg.get_action(), ArgAction::SetTrue);
    let is_integer = arg.get_value_parser().type_id() == TypeId::of::<usize>();
    let ty = if is_bool {
        "boolean"
    } else if is_integer {
        "integer"
    } else {
        "string"
    };
    let name = match arg.get_long() {
        Some(long) => long.to_string(),
        None => arg.get_id().to_string(),
    };
    let mut d = Map::new();
    d.insert("name".into(), json!(name));
    d.insert(
        "description".into(),
        json!(arg.get_help().map(ToString::to_string).unwrap_or_default()),
    );
    d.insert("type".into(), json!(ty));
    d.insert("required".into(), json!(arg.is_required_set()));
    if is_bool {
        d.insert("default".into(), json!(false));
    } else if let Some(default) = arg.get_default_values().first() {
        let text = default.to_string_lossy();
        let value = if is_integer {
            json!(text.parse::<u64>().expect("integer defaults parse"))
        } else {
            json!(text)
        };
        d.insert("default".into(), value);
    }
    if arg.get_num_args().is_some_and(|n| n.max_values() > 1) {
        d.insert("variadic".into(), json!(true));
    }
    let choices: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_string())
        .collect();
    if !is_bool && !choices.is_empty() {
        d.insert("enum".into(), json!(choices));
    }
    if matches!(arg.get_action(), ArgAction::Append) && !arg.is_positional() {
        d.insert("repeatable".into(), json!(true));
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        if defaults.is_empty() {
            d.remove("default");
        } else {
            d.insert("default".into(), json!(defaults));
        }
    }
    Value::Object(d)
}

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn nullable_string(description: &str) -> Value {
    json!({"type": ["string", "null"], "description": description})
}

fn object(required: &[&str], properties: Map<String, Value>) -> Value {
    json!({"type": "object", "required": required, "properties": properties})
}

fn props(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

const ROW_REQUIRED: [&str; 14] = [
    "id",
    "title",
    "title_source",
    "updated_at",
    "created_at",
    "size_bytes",
    "prompts",
    "is_live",
    "is_current",
    "is_pinned",
    "first_prompt",
    "cwd",
    "resume_command",
    "interactive",
];

fn row_props() -> Map<String, Value> {
    props(&[
        ("id", string("Session id, a UUID")),
        (
            "title",
            string(
                "Custom title, else AI title, else the first prompt's first 80 characters, else empty",
            ),
        ),
        (
            "title_source",
            json!({"type": "string", "enum": ["custom", "ai", "prompt", "none"]}),
        ),
        (
            "updated_at",
            string("Transcript modification time, RFC 3339 UTC with milliseconds"),
        ),
        (
            "created_at",
            nullable_string(
                "When the session started (its first entry with a working directory), RFC 3339 UTC with milliseconds; null when the transcript has none",
            ),
        ),
        (
            "size_bytes",
            json!({"type": "integer", "description": "Transcript plus its sidecar directory"}),
        ),
        (
            "prompts",
            json!({"type": "integer", "description": "Prompts a person typed; injected text is not counted"}),
        ),
        (
            "is_live",
            json!({"type": "boolean", "description": "The current session, or one a Claude Code process runs"}),
        ),
        (
            "is_current",
            json!({"type": "boolean", "description": "The session this process runs in (--current)"}),
        ),
        ("is_pinned", json!({"type": "boolean"})),
        (
            "first_prompt",
            nullable_string("The first prompt's first 300 characters, whitespace collapsed"),
        ),
        ("cwd", nullable_string("Directory the session ran in")),
        (
            "resume_command",
            string("Shell command that resumes the session where it ran"),
        ),
        (
            "interactive",
            json!({"type": ["boolean", "null"], "description": "False when the session was started by `claude -p` or the Agent SDK (an sdk-* entrypoint), null when the transcript does not say (Claude Code before 2.1.78)"}),
        ),
    ])
}

fn row() -> Value {
    object(&ROW_REQUIRED, row_props())
}

fn page(item: Value) -> Value {
    let unreadable = object(
        &["path", "message"],
        props(&[
            ("path", json!({"type": "string"})),
            ("message", json!({"type": "string"})),
        ]),
    );
    object(
        &["items", "has_more", "partial"],
        props(&[
            ("items", json!({"type": "array", "items": item})),
            ("has_more", json!({"type": "boolean"})),
            ("partial", json!({"type": "boolean"})),
            ("unreadable", json!({"type": "array", "items": unreadable})),
        ]),
    )
}

const PAGE_NOTE: &str = "partial is true when a transcript exists but could not be read: those \
    sessions are not in items, unreadable names them, and the call still prints the page but \
    exits 1 with an io_error whose context.paths lists them. unreadable is present only then. \
    partial is also true when the pins file could not be read: every session is in items, none \
    is marked pinned, and the call exits 1 with pins_unreadable after printing the page.";

fn changed() -> (&'static str, Value) {
    ("changed", json!({"type": "boolean"}))
}

fn id_changed(extra: &[(&str, Value)], required: &[&str]) -> Value {
    let mut pairs = vec![("id", string("Session id")), changed()];
    pairs.extend(extra.iter().cloned());
    let mut all = vec!["id", "changed"];
    all.extend(required);
    object(&all, props(&pairs))
}

fn paths() -> Value {
    json!({"type": "array", "items": {"type": "string"}})
}

fn contract(name: &str) -> Contract {
    let read = |output: Value, note: Option<String>| Contract {
        effects: "read_only",
        confirm: false,
        output,
        output_description: note,
        format_defaults: None,
    };
    let change =
        |effects: &'static str, confirm: bool, output: Value, note: Option<String>| Contract {
            effects,
            confirm,
            output,
            output_description: note,
            format_defaults: None,
        };
    match name {
        "list" => read(page(row()), Some(PAGE_NOTE.into())),
        "search" => {
            let mut hit = row_props();
            hit.insert("score".into(), json!({"type": "integer"}));
            hit.insert(
                "matched_in".into(),
                json!({"type": "string", "enum": ["title", "prompt", "assistant"]}),
            );
            hit.insert(
                "snippet".into(),
                string("Up to 100 characters around the match, with … on each side that was cut"),
            );
            let mut required = ROW_REQUIRED.to_vec();
            hit.insert(
                "fuzzy".into(),
                json!({"type": "boolean", "description": "The best match is a title word with a typo; such hits always rank below every exact one"}),
            );
            required.extend(["score", "matched_in", "fuzzy", "snippet"]);
            read(page(object(&required, hit)), Some(PAGE_NOTE.into()))
        }
        "get" => {
            let mut detail = row_props();
            let prompts =
                |d| json!({"type": "array", "items": {"type": "string"}, "description": d});
            detail.insert(
                "first_prompts".into(),
                prompts("Up to 3 first prompts, 300 characters each"),
            );
            detail.insert(
                "last_prompts".into(),
                prompts("Up to 3 last prompts, 300 characters each"),
            );
            detail.insert("git_branch".into(), nullable_string("Latest git branch"));
            detail.insert("model".into(), nullable_string("Latest model"));
            detail.insert(
                "version".into(),
                nullable_string("Latest Claude Code version"),
            );
            detail.insert("path".into(), string("The transcript file"));
            let mut required = ROW_REQUIRED.to_vec();
            required.extend([
                "first_prompts",
                "last_prompts",
                "git_branch",
                "model",
                "version",
                "path",
            ]);
            read(object(&required, detail), None)
        }
        "export" => Contract {
            format_defaults: Some(json!({"tty": "markdown", "non_tty": "markdown"})),
            ..read(
                object(
                    &["id", "markdown", "truncated"],
                    props(&[
                        ("id", string("Session id")),
                        (
                            "markdown",
                            string("The export, or its start when truncated"),
                        ),
                        ("truncated", json!({"type": "boolean"})),
                        ("output_file", string("File holding the whole Markdown")),
                    ]),
                ),
                Some(format!(
                    "The JSON form of the export. markdown holds at most {INLINE_MAX_BYTES} bytes; \
                     a longer export is cut there, truncated is true and output_file names \
                     <config>/sessile/exports/<id>.md, readable by its owner only, which holds \
                     the whole text until the next truncated export of the session replaces it \
                     or delete removes it. output_file is present exactly when truncated is true."
                )),
            )
        },
        "excerpts" => read(
            page(object(
                &["turn", "role", "snippet"],
                props(&[
                    (
                        "turn",
                        json!({"type": "integer", "description": "The Nth prompt and the replies after it, as export --turns numbers them; 0 for replies before the first prompt"}),
                    ),
                    (
                        "role",
                        json!({"type": "string", "enum": ["prompt", "assistant"]}),
                    ),
                    (
                        "snippet",
                        string(
                            "Up to 100 characters around the first word found, with … on each side that was cut",
                        ),
                    ),
                ]),
            )),
            Some("One session in conversation order; partial is always false.".into()),
        ),
        "rename" => change(
            "idempotent",
            false,
            id_changed(&[("title", string("The custom title now"))], &["title"]),
            None,
        ),
        "pin" | "unpin" => change(
            "idempotent",
            false,
            id_changed(&[("pinned", json!({"type": "boolean"}))], &["pinned"]),
            None,
        ),
        "archive" | "restore" => {
            let moved = object(
                &["from", "to"],
                props(&[
                    ("from", json!({"type": "string"})),
                    ("to", json!({"type": "string"})),
                ]),
            );
            change(
                "idempotent",
                false,
                id_changed(&[("moved", json!({"type": "array", "items": moved}))], &["moved"]),
                Some("moved is empty and changed false when the session was already where the command puts it.".into()),
            )
        }
        "delete" => {
            let target = object(
                &["id", "paths"],
                props(&[("id", string("Session id")), ("paths", paths())]),
            );
            change(
                "non_idempotent",
                true,
                deletion(target),
                Some(
                    "targets is the session and every path removed, or with --dry-run to be \
                      removed. requires_confirmation is present, and true, exactly with --dry-run."
                        .into(),
                ),
            )
        }
        "delete-empty" => {
            let mut target = row_props();
            target.insert("paths".into(), paths());
            let mut required = ROW_REQUIRED.to_vec();
            required.push("paths");
            let mut output = deletion(object(&required, target));
            let skipped = object(
                &["id", "reason"],
                props(&[
                    ("id", string("Session id")),
                    ("reason", json!({"type": "string"})),
                ]),
            );
            output["required"]
                .as_array_mut()
                .expect("an object schema")
                .push(json!("skipped"));
            output["properties"]["skipped"] = json!({"type": "array", "items": skipped});
            change(
                "idempotent",
                true,
                output,
                Some(
                    "targets are the empty sessions (no prompt, no reply, no attachment) \
                      removed, or with --dry-run to be removed; \
                      skipped are the pinned, current, live and unreadable ones left in place. \
                      requires_confirmation is present exactly with --dry-run: true when \
                      targets is not empty."
                        .into(),
                ),
            )
        }
        other => unreachable!("no contract for command {other}"),
    }
}

fn deletion(target: Value) -> Value {
    object(
        &["targets", "changed"],
        props(&[
            ("targets", json!({"type": "array", "items": target})),
            changed(),
            ("requires_confirmation", json!({"type": "boolean"})),
        ]),
    )
}
