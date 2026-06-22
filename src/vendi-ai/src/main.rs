//! vendi-ai — M1 headless brain.
//!
//! A local, tool-calling assistant for vendiOS. Talks to the on-box ollama
//! daemon (`/api/chat`, tool-calling) and drives a small, bounded set of tools:
//! the `vendi` control CLI, app launching, a calculator, weather/time, and file
//! reads. This binary is the `vendi ai chat` brain — single-shot for now; the
//! quickshell `super+a` panel (M2) will speak to a daemon wrapper of this loop.
//!
//! No cloud, no API keys: the only network egress is inside the `weather` tool.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::process::Command;

const OLLAMA_URL: &str = "http://127.0.0.1:11434/api/chat";
const MODEL: &str = "qwen2.5:14b-instruct";
const MAX_TOOL_TURNS: usize = 6;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prompt = if args.is_empty() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok();
        s.trim().to_string()
    } else {
        args.join(" ")
    };
    if prompt.is_empty() {
        bail!("usage: vendi-ai <prompt>   (or pipe the prompt on stdin)");
    }

    // NOTE: deliberately NO `system` message. qwen2.5 merges the tool schema into
    // the system block, and a separate system message corrupts its tool-calling
    // (intermittent empty EOS replies — verified). Guidance rides in the user turn.
    let mut messages = vec![
        json!({ "role": "user", "content": format!("{}\n\nRequest: {prompt}", preamble()) }),
    ];
    let tools = tool_defs();
    let mut empty_retries = 0;

    for _ in 0..MAX_TOOL_TURNS {
        let resp = chat(&messages, &tools).context("ollama chat call failed")?;
        let msg = resp
            .get("message")
            .cloned()
            .context("ollama response had no message")?;
        messages.push(msg.clone());

        let calls = msg.get("tool_calls").and_then(|c| c.as_array()).cloned();
        match calls {
            Some(calls) if !calls.is_empty() => {
                for c in &calls {
                    let name = c["function"]["name"].as_str().unwrap_or("");
                    let args = &c["function"]["arguments"];
                    eprintln!("  \x1b[2m→ {name}({args})\x1b[0m");
                    let result = dispatch(name, args);
                    messages.push(json!({
                        "role": "tool",
                        "tool_name": name,
                        "content": result,
                    }));
                }
                // loop again so the model can use the tool results
            }
            _ => {
                // No tool calls → final answer. Guard against an empty reply
                // (ollama can return blank content if the model was mid-load or
                // hiccuped) so we never silently "do nothing".
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("").trim();
                if content.is_empty() {
                    if empty_retries < 1 {
                        empty_retries += 1;
                        messages.pop(); // drop the blank turn and try once more
                        continue;
                    }
                    println!("Sorry — I didn't catch that. Could you say it another way?");
                    return Ok(());
                }
                println!("{content}");
                return Ok(());
            }
        }
    }
    bail!("gave up after {MAX_TOOL_TURNS} tool turns without a final answer");
}

/// One blocking round-trip to ollama (non-streaming for M1).
fn chat(messages: &[Value], tools: &Value) -> Result<Value> {
    let body = json!({
        "model": MODEL,
        "messages": messages,
        "tools": tools,
        "stream": false,
        "keep_alive": "30m",   // keep the model warm so calls stay snappy
        "options": { "temperature": 0.4 },
    });
    let resp: Value = ureq::post(OLLAMA_URL)
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("ollama request error: {e}"))?
        .into_json()?;
    if let Some(err) = resp.get("error").and_then(|e| e.as_str()) {
        bail!("ollama error: {err}");
    }
    Ok(resp)
}

/// Execute a tool call, returning a text result fed back to the model.
fn dispatch(name: &str, args: &Value) -> String {
    let r = match name {
        "calc" => calc(args["expr"].as_str().unwrap_or("")),
        "datetime" => sh("date", &["+%A %d %B %Y — %H:%M %Z"]),
        "weather" => {
            let loc = args["location"].as_str().unwrap_or("");
            if loc.is_empty() {
                sh("vendi-weather", &[])
            } else {
                sh("vendi-weather", &[loc])
            }
        }
        "launch_app" => launch_app(args),
        "run_vendi" => run_vendi(args),
        "read_file" => read_file(args["path"].as_str().unwrap_or("")),
        _ => Err(format!("unknown tool: {name}")),
    };
    match r {
        Ok(s) if s.trim().is_empty() => "(done, no output)".into(),
        Ok(s) => s,
        Err(e) => format!("error: {e}"),
    }
}

// ── tools ───────────────────────────────────────────────────────────────────

/// Precise arithmetic via awk — NEVER trust the model's mental math.
fn calc(expr: &str) -> Result<String, String> {
    if expr.is_empty() {
        return Err("empty expression".into());
    }
    // awk handles + - * / % ( ) and ^ for powers; reject anything else.
    if !expr
        .chars()
        .all(|c| c.is_ascii_digit() || " .+-*/%()^eE".contains(c))
    {
        return Err("expression contains unsupported characters".into());
    }
    sh("awk", &[&format!("BEGIN {{ printf \"%g\", ({expr}) }}")])
}

/// Launch an app, optionally opening a URL. Common sites get a known URL so the
/// model can't fumble it; otherwise the model-supplied url is used as-is.
fn launch_app(args: &Value) -> Result<String, String> {
    let app = args["app"].as_str().unwrap_or("").trim();
    if app.is_empty() {
        return Err("no app given".into());
    }
    let alias = |u: &str| -> Option<&'static str> {
        match u.to_lowercase().replace([' ', '-'], "").as_str() {
            "whatsapp" | "whatsappweb" => Some("https://web.whatsapp.com"),
            "youtube" => Some("https://youtube.com"),
            "gmail" | "mail" => Some("https://mail.google.com"),
            "github" => Some("https://github.com"),
            "maps" | "googlemaps" => Some("https://maps.google.com"),
            "chatgpt" => Some("https://chat.openai.com"),
            "twitter" | "x" => Some("https://x.com"),
            _ => None,
        }
    };
    let mut cmd = Command::new(app);
    let mut what = app.to_string();
    if let Some(url) = args["url"].as_str().filter(|u| !u.is_empty()) {
        let resolved = alias(url).map(|s| s.to_string()).unwrap_or_else(|| {
            if url.starts_with("http") {
                url.to_string()
            } else {
                format!("https://{url}")
            }
        });
        cmd.arg(&resolved);
        what = format!("{app} → {resolved}");
    }
    if let Some(extra) = args["args"].as_array() {
        for a in extra {
            if let Some(s) = a.as_str() {
                cmd.arg(s);
            }
        }
    }
    cmd.spawn().map_err(|e| format!("failed to launch {app}: {e}"))?;
    Ok(format!("launched {what}"))
}

/// Drive the `vendi` control CLI. This is the bounded system-control surface for
/// M1: the model supplies the subcommand + args (e.g. ["theme","mocha"] or
/// ["wallpaper","/path.jpg"] or ["audio","..."]); we only ever exec `vendi`.
fn run_vendi(args: &Value) -> Result<String, String> {
    let list: Vec<String> = args["args"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if list.is_empty() {
        return Err("no vendi arguments given".into());
    }
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let out = sh("vendi", &refs)?;
    Ok(if out.trim().is_empty() {
        format!("ran: vendi {}", list.join(" "))
    } else {
        out
    })
}

/// Read a (small) file so the model can answer about config/state.
fn read_file(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("no path given".into());
    }
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest)
    } else {
        path.to_string()
    };
    let data = std::fs::read_to_string(&expanded).map_err(|e| format!("cannot read {expanded}: {e}"))?;
    const CAP: usize = 8000;
    if data.len() > CAP {
        Ok(format!("{}\n…(truncated)", &data[..CAP]))
    } else {
        Ok(data)
    }
}

/// Run a command, capturing stdout (and stderr on failure) as a string.
fn sh(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {bin}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{bin} failed: {}", err.trim()))
    }
}

// ── tool schemas ─────────────────────────────────────────────────────────────

fn tool_defs() -> Value {
    json!([
        f("calc", "Evaluate a precise arithmetic expression. ALWAYS use this for any math instead of computing it yourself. Translate the user's question into an expression, e.g. \"18% of 2340\" -> \"2340*18/100\".", json!({
            "type":"object",
            "properties": { "expr": { "type":"string", "description":"arithmetic expression with + - * / % ( ) ^" } },
            "required": ["expr"]
        })),
        f("datetime", "Get the current local date and time.", json!({ "type":"object", "properties": {} })),
        f("weather", "Get the current weather. Optionally for a given location (city or \"lat,lon\"); omit for the user's location.", json!({
            "type":"object",
            "properties": { "location": { "type":"string" } }
        })),
        f("launch_app", "Open a desktop application, optionally with a URL (e.g. app=firefox, url=whatsapp web).", json!({
            "type":"object",
            "properties": {
                "app": { "type":"string", "description":"executable name, e.g. firefox" },
                "url": { "type":"string", "description":"optional site/URL to open" },
                "args": { "type":"array", "items": { "type":"string" } }
            },
            "required": ["app"]
        })),
        f("run_vendi", "Control vendiOS appearance/system via the `vendi` CLI. Pass the subcommand and its arguments as a list. Subcommands: theme <name> (themes: mocha, and others; `theme list` to see), wallpaper <path>, appearance light|dark, bar ..., audio, wifi, bt, power, display, screensaver, update, info. e.g. switch theme -> [\"theme\",\"mocha\"].", json!({
            "type":"object",
            "properties": { "args": { "type":"array", "items": { "type":"string" } } },
            "required": ["args"]
        })),
        f("read_file", "Read a (small) text file, e.g. a config under ~/.config/vendi/.", json!({
            "type":"object",
            "properties": { "path": { "type":"string" } },
            "required": ["path"]
        })),
    ])
}

fn f(name: &str, desc: &str, params: Value) -> Value {
    json!({ "type":"function", "function": { "name": name, "description": desc, "parameters": params } })
}

// ── context ──────────────────────────────────────────────────────────────────

/// Guidance prepended to the user's request (NOT a system message — see the note
/// at the call site). Kept compact: long instructions here also hurt reliability.
fn preamble() -> String {
    let theme = std::fs::read_to_string(format!(
        "{}/.config/vendi/theme-state",
        std::env::var("HOME").unwrap_or_default()
    ))
    .ok()
    .and_then(|s| {
        s.lines()
            .find_map(|l| l.trim().strip_prefix("THEME=").map(|v| v.trim().trim_matches('"').to_string()))
    })
    .unwrap_or_else(|| "unknown".into());

    format!(
        "You are vendi AI, the local assistant for vendiOS (a clean Arch setup with \
the vendiwm compositor + quickshell bar). Use the tools to actually DO things — \
appearance via run_vendi, math via calc, weather/time via their tools, apps via \
launch_app — never just acknowledge, and never invent shell commands. You are not \
a coding assistant. Keep replies short and friendly. Current theme: {theme}."
    )
}
