//! vendi-ai — local tool-calling assistant brain for vendiOS.
//!
//! Talks to the on-box ollama daemon (`/api/chat`, tool-calling, STREAMING) and
//! drives a small bounded toolset: the `vendi` control CLI, app launching, a
//! calculator, weather/time, a keyless web search, and file reads. Single-shot
//! per invocation; the quickshell `super+a` panel calls it and shows the answer
//! as it streams.
//!
//! Streaming: final-answer tokens are printed to stdout AS THEY ARRIVE. On a TTY
//! they print raw (clean in a terminal); when piped (the panel) each chunk is
//! followed by a 0x1e record separator so the reader can reassemble while
//! preserving real newlines.
//!
//! No cloud, no API keys: network egress happens only inside the web/weather
//! tools. NOTE: NO system message — qwen2.5 merges tools into the system block
//! and a separate system message corrupts its tool-calling (empty replies).
//! Guidance rides in the user turn.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Read, Write};
use std::process::Command;

const OLLAMA_URL: &str = "http://127.0.0.1:11434/api/chat";
const MODEL: &str = "qwen2.5:14b-instruct";
const MAX_TOOL_TURNS: usize = 6;
const SEP: char = '\u{1e}'; // record separator between streamed chunks (piped mode)

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

    let tty = std::io::stdout().is_terminal();
    let mut messages = vec![
        json!({ "role": "user", "content": format!("{}\n\nRequest: {prompt}", preamble()) }),
    ];
    let tools = tool_defs();
    let mut streamed_any = false;
    let mut empty_retries = 0;

    for _ in 0..MAX_TOOL_TURNS {
        let (content, tool_calls) = chat_stream(&messages, &tools, &mut |chunk| {
            streamed_any = true;
            emit(chunk, tty);
        })?;

        // record this assistant turn
        let mut asst = json!({ "role": "assistant", "content": content });
        if !tool_calls.is_empty() {
            asst["tool_calls"] = Value::Array(tool_calls.clone());
        }
        messages.push(asst);

        if !tool_calls.is_empty() {
            for c in &tool_calls {
                let name = c["function"]["name"].as_str().unwrap_or("");
                let cargs = &c["function"]["arguments"];
                eprintln!("  \x1b[2m→ {name}({cargs})\x1b[0m");
                // Tier-2 actions (shell, power, update, …) ask the user first.
                if let Some((title, detail)) = needs_permission(name, cargs) {
                    if !request_permission(&title, &detail, tty) {
                        messages.push(json!({ "role": "tool", "tool_name": name,
                            "content": "The user DENIED permission for this action. Do not perform it; briefly tell them it was cancelled." }));
                        continue;
                    }
                }
                let (result, card) = dispatch(name, cargs);
                if let Some(cj) = card {
                    emit_card(&cj, tty);
                }
                messages.push(json!({ "role": "tool", "tool_name": name, "content": result }));
            }
            continue; // let the model use the results
        }

        // no tool calls → this turn was the final answer
        if content.trim().is_empty() {
            if empty_retries < 1 {
                empty_retries += 1;
                messages.pop(); // drop the blank turn, retry once
                continue;
            }
            emit("Sorry — I didn't catch that. Could you say it another way?", tty);
        }
        if tty {
            println!();
        }
        let _ = std::io::stdout().flush();
        return Ok(());
    }
    if !streamed_any {
        emit("Sorry — I got stuck on that one.", tty);
    }
    if tty {
        println!();
    }
    Ok(())
}

/// Print a streamed chunk: raw on a TTY, separator-terminated when piped.
fn emit(chunk: &str, tty: bool) {
    let mut out = std::io::stdout();
    if tty {
        let _ = write!(out, "{chunk}");
    } else {
        let _ = write!(out, "{chunk}{SEP}");
    }
    let _ = out.flush();
}

/// Streaming round-trip to ollama. Calls `on_chunk` for each content token as it
/// arrives; returns the full assistant content + any tool calls.
fn chat_stream(
    messages: &[Value],
    tools: &Value,
    on_chunk: &mut dyn FnMut(&str),
) -> Result<(String, Vec<Value>)> {
    let body = json!({
        "model": MODEL,
        "messages": messages,
        "tools": tools,
        "stream": true,
        "keep_alive": "30m",
        "options": { "temperature": 0.4 },
    });
    let resp = ureq::post(OLLAMA_URL)
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("ollama request error: {e}"))?;

    let mut content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let reader = std::io::BufReader::new(resp.into_reader());
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)?;
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            bail!("ollama error: {e}");
        }
        if let Some(msg) = v.get("message") {
            if let Some(tc) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                tool_calls.extend(tc.iter().cloned());
            }
            if let Some(c) = msg.get("content").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    content.push_str(c);
                    on_chunk(c);
                }
            }
        }
        if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            break;
        }
    }
    Ok((content, tool_calls))
}

/// Execute a tool call. Returns (text result fed back to the model, optional UI
/// card JSON rendered in the panel).
fn dispatch(name: &str, args: &Value) -> (String, Option<String>) {
    match name {
        "weather" => return weather_tool(),
        "show_card" => return show_card_tool(args),
        "show_match" => return show_match_tool(args),
        _ => {}
    }
    let r = match name {
        "calc" => calc(args["expr"].as_str().unwrap_or("")),
        "datetime" => sh("date", &["+%A %d %B %Y — %H:%M %Z"]),
        "web_search" => web_search(args["query"].as_str().unwrap_or("")),
        "launch_app" => launch_app(args),
        "run_vendi" => run_vendi(args),
        "run_command" => {
            let cmd = args["command"].as_str().unwrap_or("");
            if cmd.is_empty() { Err("no command".into()) } else { sh("sh", &["-c", cmd]) }
        }
        "read_file" => read_file(args["path"].as_str().unwrap_or("")),
        _ => Err(format!("unknown tool: {name}")),
    };
    let text = match r {
        Ok(s) if s.trim().is_empty() => "(done, no output)".into(),
        Ok(s) => s,
        Err(e) => format!("error: {e}"),
    };
    (text, None)
}

/// Print a card directive to the panel: the [[CARD]] tag + single-line JSON +
/// separator. A plain-text tag (not a control byte) so the panel's SplitParser
/// doesn't trim it. Skipped on a TTY (cards only render in the GUI panel).
fn emit_card(json: &str, tty: bool) {
    if tty {
        return;
    }
    let mut out = std::io::stdout();
    let _ = write!(out, "[[CARD]]{json}{SEP}");
    let _ = out.flush();
}

/// Does this tool call need explicit user permission (Tier 2)? Returns a
/// (title, detail) to show on the permission card, or None for auto-run actions.
fn needs_permission(name: &str, args: &Value) -> Option<(String, String)> {
    match name {
        "run_command" => {
            let c = args["command"].as_str().unwrap_or("");
            Some(("Run a shell command".to_string(), c.to_string()))
        }
        "run_vendi" => {
            let list: Vec<String> = args["args"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let sub = list.first().map(String::as_str).unwrap_or("");
            if matches!(sub, "power" | "update" | "rollback" | "clean" | "snapshot") {
                Some((format!("Run: vendi {}", list.join(" ")), "System action".to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Ask the user to approve a Tier-2 action. On the panel (piped) we emit a
/// [[PERM]] directive and block reading the decision from stdin (the panel
/// writes "allow"/"deny"). On a TTY we prompt y/N on stderr.
fn request_permission(title: &str, detail: &str, tty: bool) -> bool {
    if tty {
        eprint!("  \x1b[33m[permission]\x1b[0m {title}\n  {detail}\n  allow? [y/N] ");
        let _ = std::io::stderr().flush();
    } else {
        let payload = json!({ "title": title, "detail": detail }).to_string();
        let mut out = std::io::stdout();
        let _ = write!(out, "[[PERM]]{payload}{SEP}");
        let _ = out.flush();
    }
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let d = line.trim().to_lowercase();
    d == "allow" || d == "y" || d == "yes"
}

/// Weather → a rich weather card (city, temp, condition, high/low).
fn weather_tool() -> (String, Option<String>) {
    match sh("vendi-weather", &["--card"]) {
        Ok(line) => {
            // city|emoji|temp°C|cond|hi°|lo°
            let p: Vec<&str> = line.split('|').collect();
            let g = |i: usize| p.get(i).copied().unwrap_or("").trim();
            let (city, icon, temp, cond, hi, lo) = (g(0), g(1), g(2), g(3), g(4), g(5));
            let card = json!({
                "type": "weather", "city": city, "icon": icon,
                "temp": temp, "cond": cond, "hi": hi, "lo": lo
            })
            .to_string();
            let where_ = if city.is_empty() { String::new() } else { format!(" in {city}") };
            let text = format!("Weather{where_}: {temp}, {cond} (high {hi}, low {lo}).");
            (text, Some(card))
        }
        Err(e) => (format!("error: {e}"), None),
    }
}

/// Stringify a JSON value that may be a string or a number (model may send either).
fn vstr(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        s.to_string()
    } else if v.is_number() {
        v.to_string()
    } else {
        String::new()
    }
}

/// National-team name (any language) → 🇨🇨 flag emoji, so flags show even when
/// the model doesn't supply them. Empty for clubs / unknown.
fn country_flag(name: &str) -> String {
    let n = name.trim().to_lowercase();
    let iso = match n.as_str() {
        "spain" | "españa" | "espana" => "ES",
        "saudi arabia" | "arabia saudita" | "arabia saudí" => "SA",
        "france" | "francia" => "FR",
        "argentina" => "AR",
        "brazil" | "brasil" => "BR",
        "england" | "inglaterra" | "wales" | "gales" | "scotland" | "escocia" => "GB",
        "germany" | "alemania" => "DE",
        "portugal" => "PT",
        "italy" | "italia" => "IT",
        "netherlands" | "holanda" | "países bajos" => "NL",
        "usa" | "united states" | "estados unidos" => "US",
        "mexico" | "méxico" => "MX",
        "morocco" | "marruecos" => "MA",
        "japan" | "japón" | "japon" => "JP",
        "croatia" | "croacia" => "HR",
        "belgium" | "bélgica" | "belgica" => "BE",
        "uruguay" => "UY",
        "colombia" => "CO",
        "switzerland" | "suiza" => "CH",
        "denmark" | "dinamarca" => "DK",
        "poland" | "polonia" => "PL",
        "senegal" => "SN",
        "south korea" | "korea" | "corea" | "corea del sur" => "KR",
        "ecuador" => "EC",
        "ghana" => "GH",
        "cameroon" | "camerún" | "camerun" => "CM",
        "serbia" => "RS",
        "qatar" | "catar" => "QA",
        "canada" | "canadá" => "CA",
        "australia" => "AU",
        "tunisia" | "túnez" | "tunez" => "TN",
        "costa rica" => "CR",
        "iran" | "irán" => "IR",
        "nigeria" => "NG",
        "egypt" | "egipto" => "EG",
        "chile" => "CL",
        "peru" | "perú" => "PE",
        "sweden" | "suecia" => "SE",
        "norway" | "noruega" => "NO",
        "austria" => "AT",
        "turkey" | "turquía" | "turquia" => "TR",
        "greece" | "grecia" => "GR",
        "ireland" | "irlanda" => "IE",
        _ => return String::new(),
    };
    iso.chars()
        .filter_map(|c| char::from_u32(0x1F1E6 + (c.to_ascii_uppercase() as u32 - 'A' as u32)))
        .collect()
}

/// show_match → a football/sports match-result card.
fn show_match_tool(args: &Value) -> (String, Option<String>) {
    let home = args["home"].as_str().unwrap_or("");
    let away = args["away"].as_str().unwrap_or("");
    let hs = vstr(&args["home_score"]);
    let as_ = vstr(&args["away_score"]);
    // prefer a model-supplied emoji, else derive from the country name
    let home_flag = {
        let f = args["home_flag"].as_str().unwrap_or("");
        if f.is_empty() { country_flag(home) } else { f.to_string() }
    };
    let away_flag = {
        let f = args["away_flag"].as_str().unwrap_or("");
        if f.is_empty() { country_flag(away) } else { f.to_string() }
    };
    let card = json!({
        "type": "match",
        "home": home, "away": away, "hs": hs, "as": as_,
        "homeFlag": home_flag,
        "awayFlag": away_flag,
        "comp": args["competition"].as_str().unwrap_or(""),
        "status": args["status"].as_str().unwrap_or(""),
        "stage": args["stage"].as_str().unwrap_or(""),
        "scorers": args["scorers"].clone(),
    })
    .to_string();
    (format!("Showed match card: {home} {hs}-{as_} {away}"), Some(card))
}

/// show_card → render a generic info card (stats, comparisons, rankings…).
fn show_card_tool(args: &Value) -> (String, Option<String>) {
    let title = args["title"].as_str().unwrap_or("");
    if title.is_empty() && !args["rows"].is_array() {
        return ("error: show_card needs a title and rows".into(), None);
    }
    let card = json!({
        "type": "info",
        "title": title,
        "subtitle": args["subtitle"].as_str().unwrap_or(""),
        "accent": args["accent"].as_str().unwrap_or(""),
        "rows": args["rows"].clone(),
    })
    .to_string();
    (format!("Displayed a card: {title}"), Some(card))
}

// ── tools ───────────────────────────────────────────────────────────────────

fn calc(expr: &str) -> Result<String, String> {
    if expr.is_empty() {
        return Err("empty expression".into());
    }
    if !expr.chars().all(|c| c.is_ascii_digit() || " .+-*/%()^eE".contains(c)) {
        return Err("expression contains unsupported characters".into());
    }
    sh("awk", &[&format!("BEGIN {{ printf \"%g\", ({expr}) }}")])
}

/// Keyless web search. Scrapes DuckDuckGo's HTML endpoint for real result
/// snippets (titles + descriptions) — far more useful than the instant-answer
/// API for dates/events/recent facts. Falls back to the IA API abstract.
fn web_search(query: &str) -> Result<String, String> {
    if query.trim().is_empty() {
        return Err("empty query".into());
    }
    // ── primary: DDG HTML results ──
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));
    let html = ureq::get(&url)
        .set("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:124.0) Gecko/20100101 Firefox/124.0")
        .call()
        .map_err(|e| format!("search request failed: {e}"))
        .and_then(|r| r.into_string().map_err(|e| format!("search read failed: {e}")));

    if let Ok(body) = html {
        let mut results: Vec<String> = Vec::new();
        // pair each result title with its snippet
        let titles = extract_class(&body, "result__a", 5);
        let snips = extract_class(&body, "result__snippet", 5);
        for i in 0..snips.len().max(titles.len()).min(4) {
            let t = titles.get(i).cloned().unwrap_or_default();
            let s = snips.get(i).cloned().unwrap_or_default();
            let line = match (t.is_empty(), s.is_empty()) {
                (false, false) => format!("• {t} — {s}"),
                (true, false) => format!("• {s}"),
                (false, true) => format!("• {t}"),
                _ => continue,
            };
            results.push(line);
        }
        if !results.is_empty() {
            return Ok(results.join("\n"));
        }
    }

    // ── fallback: DDG instant-answer abstract ──
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&no_redirect=1&t=vendi",
        urlencode(query)
    );
    if let Ok(resp) = ureq::get(&url).call() {
        if let Ok(v) = resp.into_json::<Value>() {
            let abs = v["AbstractText"].as_str().unwrap_or("");
            if !abs.is_empty() {
                return Ok(abs.to_string());
            }
        }
    }
    Ok(format!("No clear web result for \"{query}\". Tell the user you're not certain."))
}

/// Pull the inner text of the first `n` elements carrying `class="…<name>…"`
/// from a blob of HTML, stripping tags and decoding a few entities.
fn extract_class(html: &str, name: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for part in html.split(name).skip(1) {
        if let Some(gt) = part.find('>') {
            let after = &part[gt + 1..];
            if let Some(end) = after.find("</a>") {
                let text = strip_tags(&after[..end]);
                let text = text.trim();
                if !text.is_empty() {
                    out.push(text.to_string());
                }
                if out.len() >= n {
                    break;
                }
            }
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

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
        let resolved = alias(url).map(String::from).unwrap_or_else(|| {
            if url.starts_with("http") { url.to_string() } else { format!("https://{url}") }
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
    Ok(if out.trim().is_empty() { format!("ran: vendi {}", list.join(" ")) } else { out })
}

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

fn sh(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {bin}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("{bin} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Minimal percent-encoding for a query string.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── tool schemas ─────────────────────────────────────────────────────────────

fn tool_defs() -> Value {
    json!([
        f("calc", "Evaluate a precise arithmetic expression. ALWAYS use this for math instead of computing yourself, e.g. \"18% of 2340\" -> \"2340*18/100\".", json!({
            "type":"object", "properties": { "expr": { "type":"string" } }, "required": ["expr"]
        })),
        f("datetime", "Get the current local date and time.", json!({ "type":"object", "properties": {} })),
        f("weather", "Get the current weather (optionally for a city or \"lat,lon\").", json!({
            "type":"object", "properties": { "location": { "type":"string" } }
        })),
        f("web_search", "Search the web for facts you don't know or that may be recent (dates, events, \"what/who is X\", current info). Use this whenever you are unsure rather than guessing.", json!({
            "type":"object", "properties": { "query": { "type":"string" } }, "required": ["query"]
        })),
        f("launch_app", "Open a desktop app, optionally with a URL (e.g. app=firefox, url=whatsapp web).", json!({
            "type":"object",
            "properties": {
                "app": { "type":"string" }, "url": { "type":"string" },
                "args": { "type":"array", "items": { "type":"string" } }
            }, "required": ["app"]
        })),
        f("run_vendi", "Control vendiOS appearance/system via the `vendi` CLI; pass the subcommand + args as a list. Subcommands: theme <name> (mocha latte gruvbox mono think dynamic; `theme list`), wallpaper <path>, appearance light|dark, bar, audio, wifi, bt, power, display, screensaver, update, info. e.g. switch theme -> [\"theme\",\"mocha\"].", json!({
            "type":"object", "properties": { "args": { "type":"array", "items": { "type":"string" } } }, "required": ["args"]
        })),
        f("read_file", "Read a small text file, e.g. a config under ~/.config/vendi/.", json!({
            "type":"object", "properties": { "path": { "type":"string" } }, "required": ["path"]
        })),
        f("show_match", "Display a FOOTBALL/SPORTS MATCH RESULT card. Use this for any game score. Give both teams, both scores, the competition (e.g. \"FIFA World Cup 2026 · Yesterday\"), status (e.g. \"Full time\") and stage (e.g. \"Group stage · Group H\"). ALWAYS fill `scorers` with EVERY goal — one array entry per goal as \"Player MIN'\" (e.g. \"Lamine Yamal 10'\", \"Oyarzabal 21'\", \"Oyarzabal 24'\", \"Tambakti 49' (OG)\"). The TOTAL number of goals listed MUST equal home_score + away_score (include own goals, marked (OG)). Search the web for the goalscorers if you don't know them. Flags are added automatically from the team names for national teams; omit home_flag/away_flag.", json!({
            "type":"object",
            "properties": {
                "home": { "type":"string" }, "away": { "type":"string" },
                "home_score": { "type":"string" }, "away_score": { "type":"string" },
                "home_flag": { "type":"string" }, "away_flag": { "type":"string" },
                "competition": { "type":"string" }, "status": { "type":"string" },
                "stage": { "type":"string" },
                "scorers": { "type":"array", "items": { "type":"string" } }
            },
            "required": ["home","away","home_score","away_score"]
        })),
        f("show_card", "Display a nice visual card in the UI for OTHER structured results — stats, comparisons, rankings, summaries (NOT match scores — use show_match for those). Give a title, an optional subtitle, and rows of label/value pairs. Still also write a short text reply.", json!({
            "type":"object",
            "properties": {
                "title": { "type":"string", "description":"e.g. \"Real Madrid 2 – 1 Barcelona\"" },
                "subtitle": { "type":"string", "description":"e.g. \"LaLiga · Full time\"" },
                "rows": { "type":"array", "items": { "type":"object",
                    "properties": { "label": { "type":"string" }, "value": { "type":"string" } } } },
                "accent": { "type":"string", "description":"optional hex colour like #4caf50" }
            },
            "required": ["title"]
        })),
        f("run_command", "Run an arbitrary shell command for things no other tool covers (managing packages, files, processes…). The user is shown the exact command and must approve it first. Prefer the specific tools (run_vendi, launch_app) when they fit.", json!({
            "type":"object", "properties": { "command": { "type":"string" } }, "required": ["command"]
        })),
    ])
}

fn f(name: &str, desc: &str, params: Value) -> Value {
    json!({ "type":"function", "function": { "name": name, "description": desc, "parameters": params } })
}

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
launch_app — never just acknowledge, and never invent shell commands. If you are \
unsure of a fact or it might be recent (dates, events, who/what is X), call \
web_search instead of guessing. For a football/sports match score, call \
show_match; for other structured results (stats, comparisons, rankings) call \
show_card; both display a nice card. The card is shown to the user AUTOMATICALLY \
below your reply — so do NOT describe it, link it, or say \"here is the card\"; \
just give ONE short sentence of plain text. Never use markdown (no **bold**, no \
![](image links), no headings). You are not a coding assistant. Be brief and \
friendly. Current theme: {theme}."
    )
}
