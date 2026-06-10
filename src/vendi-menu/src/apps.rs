// .desktop application index.
//
// Scans the standard XDG application dirs, keeps Name/Exec/Icon/Comment,
// skips NoDisplay/Hidden entries. Search is a ranked substring match:
// name-prefix beats name-substring beats comment/keyword hits.

use std::path::Path;

#[derive(Debug, Clone)]
pub struct DesktopApp {
    pub name:     String,
    pub exec:     String,
    pub icon:     Option<String>,
    pub comment:  Option<String>,
    pub keywords: String,   // lowercase, for matching only
}

pub fn load() -> Vec<DesktopApp> {
    let mut dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/applications".into(),
        "/usr/local/share/applications".into(),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/applications"));
    }

    let mut out: Vec<DesktopApp> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") { continue; }
            if let Some(app) = parse_desktop_file(&path) {
                // Later dirs (user-local) override earlier ones by name.
                if let Some(existing) = out.iter_mut().find(|a| a.name == app.name) {
                    *existing = app;
                } else {
                    out.push(app);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn parse_desktop_file(path: &Path) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_main = false;
    let (mut name, mut exec, mut icon, mut comment, mut keywords) =
        (None, None, None, None, String::new());
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main { continue; }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "NoDisplay" | "Hidden" if value == "true" => return None,
            "Type" if value != "Application" => return None,
            "Name"     if name.is_none() => name = Some(value.to_string()),
            "Exec"     if exec.is_none() => exec = Some(clean_exec(value)),
            "Icon"     if icon.is_none() => icon = Some(value.to_string()),
            "Comment"  if comment.is_none() => comment = Some(value.to_string()),
            "Keywords" => keywords = value.to_lowercase(),
            _ => {}
        }
    }
    Some(DesktopApp {
        name: name?,
        exec: exec?,
        icon,
        comment,
        keywords,
    })
}

/// Strip the %u/%f/%U/%F field codes the spec allows in Exec lines.
fn clean_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|tok| !tok.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ranked search. Lower score = better.
pub fn search(index: &[DesktopApp], query: &str, limit: usize) -> Vec<DesktopApp> {
    let mut hits: Vec<(u8, &DesktopApp)> = index.iter()
        .filter_map(|app| {
            let name = app.name.to_lowercase();
            let score = if name.starts_with(query)          { 0 }
                else if name.contains(query)                { 1 }
                else if app.keywords.contains(query)        { 2 }
                else if app.comment.as_deref()
                    .map(|c| c.to_lowercase().contains(query))
                    .unwrap_or(false)                       { 3 }
                else { return None };
            Some((score, app))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.len().cmp(&b.1.name.len())));
    hits.into_iter().take(limit).map(|(_, a)| a.clone()).collect()
}
