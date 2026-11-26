#![allow(clippy::print_stdout)]

use crate::ui;
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_TOOL_TURNS: usize = 16;
const MAX_TOOL_OUTPUT_BYTES: usize = 24 * 1024;

/// Mode eksekusi tool pada native agent Latch (serupa `antigravity-cli`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    Confirm,
    AcceptEdits,
    Auto,
    Plan,
}

impl ExecMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::AcceptEdits => "accept-edits",
            Self::Auto => "auto",
            Self::Plan => "plan",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "confirm" => Some(Self::Confirm),
            "accept-edits" => Some(Self::AcceptEdits),
            "auto" => Some(Self::Auto),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }
}

/// Penyedia LLM untuk native agent bawaan Latch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
    OpenRouter,
    Gemini,
}

impl Provider {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Gemini => "gemini",
        }
    }

    #[must_use]
    pub const fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-sonnet-4-20250514",
            Self::OpenAi => "gpt-4o",
            Self::OpenRouter => "anthropic/claude-sonnet-4",
            Self::Gemini => "gemini-2.5-pro",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderAuth {
    pub provider: Provider,
    pub api_key: String,
    pub base_url: Option<String>,
}

/// Mendeteksi provider LLM aktif dari environment variable API key.
#[must_use]
pub fn detect_provider() -> Option<ProviderAuth> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        && !key.trim().is_empty()
    {
        return Some(ProviderAuth {
            provider: Provider::Anthropic,
            api_key: key,
            base_url: std::env::var("ANTHROPIC_BASE_URL").ok(),
        });
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY")
        && !key.trim().is_empty()
    {
        return Some(ProviderAuth {
            provider: Provider::OpenAi,
            api_key: key,
            base_url: std::env::var("OPENAI_BASE_URL").ok(),
        });
    }
    if let Ok(key) = std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
        && !key.trim().is_empty()
    {
        return Some(ProviderAuth {
            provider: Provider::Gemini,
            api_key: key,
            base_url: None,
        });
    }
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY")
        && !key.trim().is_empty()
    {
        return Some(ProviderAuth {
            provider: Provider::OpenRouter,
            api_key: key,
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
        });
    }
    None
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// State percakapan multi-turn untuk native agent Latch.
#[derive(Debug, Default)]
pub struct NativeAgentState {
    pub model_override: Option<String>,
    pub messages: Vec<ChatMessage>,
}

impl NativeAgentState {
    pub fn reset(&mut self) {
        self.messages.clear();
    }
}

struct ToolCall {
    id: String,
    name: String,
    args: Value,
}

enum LlmTurn {
    Text(String),
    Tools {
        text: Option<String>,
        calls: Vec<ToolCall>,
    },
}

/// Menjalankan satu giliran (multi-step tool loop) native Rust agent di dalam `workdir`.
pub fn run_native_turn(
    state: &mut NativeAgentState,
    workdir: &Path,
    mode: ExecMode,
    user_prompt: &str,
) -> anyhow::Result<()> {
    let auth = detect_provider().ok_or_else(|| {
        anyhow::anyhow!(
            "No LLM API key found for built-in `latch` agent.\n  Set ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, or OPENROUTER_API_KEY — or switch to an OAuth CLI agent via `/agent` (e.g. antigravity, codex, opencode)."
        )
    })?;

    let model = state
        .model_override
        .clone()
        .unwrap_or_else(|| auth.provider.default_model().to_string());

    ui::print_info(&format!(
        "latch native agent · {} ({}) · mode: {}",
        auth.provider.as_str(),
        model,
        mode.as_str()
    ));

    state.messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_prompt.to_string(),
    });

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("latch-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let system_prompt = format!(
        "You are Latch, a concise, accurate autonomous coding agent operating inside workspace: {}.\n\
         Execution mode: {}. Use tools to inspect and edit files cleanly. Keep explanations brief.",
        workdir.display(),
        mode.as_str()
    );

    let mut tool_history: Vec<(ToolCall, String)> = Vec::new();

    for _step in 0..MAX_TOOL_TURNS {
        let turn = rt.block_on(call_llm(
            &client,
            &auth,
            &model,
            &system_prompt,
            &state.messages,
            &tool_history,
        ))?;

        match turn {
            LlmTurn::Text(reply) => {
                println!();
                for line in reply.lines() {
                    println!("  {line}");
                }
                state.messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: reply,
                });
                return Ok(());
            }
            LlmTurn::Tools { text, calls } => {
                if let Some(t) = text
                    && !t.trim().is_empty()
                {
                    println!();
                    for line in t.lines() {
                        println!("  {line}");
                    }
                }
                for call in calls {
                    let result = execute_tool(workdir, mode, &call.name, &call.args);
                    tool_history.push((call, result));
                }
            }
        }
    }

    ui::print_warn("Reached maximum tool iterations for this turn.");
    Ok(())
}

async fn call_llm(
    client: &reqwest::Client,
    auth: &ProviderAuth,
    model: &str,
    system: &str,
    messages: &[ChatMessage],
    tool_history: &[(ToolCall, String)],
) -> anyhow::Result<LlmTurn> {
    match auth.provider {
        Provider::Anthropic => {
            call_anthropic(client, auth, model, system, messages, tool_history).await
        }
        Provider::OpenAi | Provider::OpenRouter => {
            call_openai_compat(client, auth, model, system, messages, tool_history).await
        }
        Provider::Gemini => call_gemini(client, auth, model, system, messages, tool_history).await,
    }
}

fn tool_schemas_openai() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List files in the workspace directory.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Optional relative subdirectory" } }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file from the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a file in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace an exact substring in an existing workspace file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "target": { "type": "string" },
                        "replacement": { "type": "string" }
                    },
                    "required": ["path", "target", "replacement"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_text",
                "description": "Search for a plain-text substring across workspace files.",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command inside the workspace directory.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"]
                }
            }
        }
    ])
}

fn tool_schemas_anthropic() -> Value {
    let openai = tool_schemas_openai();
    let arr = openai
        .as_array()
        .map(|list| {
            list.iter()
                .map(|item| {
                    let f = &item["function"];
                    json!({
                        "name": f["name"],
                        "description": f["description"],
                        "input_schema": f["parameters"],
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Value::Array(arr)
}

async fn call_openai_compat(
    client: &reqwest::Client,
    auth: &ProviderAuth,
    model: &str,
    system: &str,
    messages: &[ChatMessage],
    tool_history: &[(ToolCall, String)],
) -> anyhow::Result<LlmTurn> {
    let base = auth
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/');
    let url = format!("{base}/chat/completions");

    let mut api_msgs = vec![json!({ "role": "system", "content": system })];
    for m in messages {
        api_msgs.push(json!({ "role": m.role, "content": m.content }));
    }
    for (call, output) in tool_history {
        api_msgs.push(json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": [{
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.args.to_string(),
                }
            }]
        }));
        api_msgs.push(json!({
            "role": "tool",
            "tool_call_id": call.id,
            "content": output,
        }));
    }

    let body = json!({
        "model": model,
        "messages": api_msgs,
        "tools": tool_schemas_openai(),
    });

    let resp = client
        .post(&url)
        .bearer_auth(&auth.api_key)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("LLM API error ({status}): {val}");
    }

    let msg = &val["choices"][0]["message"];
    let text = msg["content"].as_str().map(ToString::to_string);
    if let Some(tcs) = msg["tool_calls"].as_array()
        && !tcs.is_empty()
    {
        let mut calls = Vec::new();
        for tc in tcs {
            let id = tc["id"].as_str().unwrap_or("call_0").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let raw_args = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let args: Value = serde_json::from_str(raw_args).unwrap_or_else(|_| json!({}));
            calls.push(ToolCall { id, name, args });
        }
        return Ok(LlmTurn::Tools { text, calls });
    }

    Ok(LlmTurn::Text(text.unwrap_or_default()))
}

async fn call_anthropic(
    client: &reqwest::Client,
    auth: &ProviderAuth,
    model: &str,
    system: &str,
    messages: &[ChatMessage],
    tool_history: &[(ToolCall, String)],
) -> anyhow::Result<LlmTurn> {
    let base = auth
        .base_url
        .as_deref()
        .unwrap_or("https://api.anthropic.com")
        .trim_end_matches('/');
    let url = format!("{base}/v1/messages");

    let mut api_msgs: Vec<Value> = messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    for (call, output) in tool_history {
        api_msgs.push(json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.args,
            }]
        }));
        api_msgs.push(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": call.id,
                "content": output,
            }]
        }));
    }

    let body = json!({
        "model": model,
        "max_tokens": 4096,
        "system": system,
        "messages": api_msgs,
        "tools": tool_schemas_anthropic(),
    });

    let resp = client
        .post(&url)
        .header("x-api-key", &auth.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("Anthropic API error ({status}): {val}");
    }

    let mut texts = Vec::new();
    let mut calls = Vec::new();
    if let Some(blocks) = val["content"].as_array() {
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => {
                    if let Some(t) = b["text"].as_str() {
                        texts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    calls.push(ToolCall {
                        id: b["id"].as_str().unwrap_or("tu_0").to_string(),
                        name: b["name"].as_str().unwrap_or("").to_string(),
                        args: b["input"].clone(),
                    });
                }
                _ => {}
            }
        }
    }

    let combined = texts.join("\n");
    if !calls.is_empty() {
        Ok(LlmTurn::Tools {
            text: if combined.is_empty() {
                None
            } else {
                Some(combined)
            },
            calls,
        })
    } else {
        Ok(LlmTurn::Text(combined))
    }
}

async fn call_gemini(
    client: &reqwest::Client,
    auth: &ProviderAuth,
    model: &str,
    system: &str,
    messages: &[ChatMessage],
    tool_history: &[(ToolCall, String)],
) -> anyhow::Result<LlmTurn> {
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");

    let mut contents: Vec<Value> = messages
        .iter()
        .map(|m| {
            let role = if m.role == "assistant" {
                "model"
            } else {
                "user"
            };
            json!({ "role": role, "parts": [{ "text": m.content }] })
        })
        .collect();

    for (call, output) in tool_history {
        contents.push(json!({
            "role": "model",
            "parts": [{ "functionCall": { "name": call.name, "args": call.args } }]
        }));
        contents.push(json!({
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": call.name,
                    "response": { "result": output }
                }
            }]
        }));
    }

    let decls: Vec<Value> = tool_schemas_openai()
        .as_array()
        .map(|list| {
            list.iter()
                .map(|item| {
                    let f = &item["function"];
                    json!({
                        "name": f["name"],
                        "description": f["description"],
                        "parameters": f["parameters"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let body = json!({
        "system_instruction": { "parts": [{ "text": system }] },
        "contents": contents,
        "tools": [{ "function_declarations": decls }],
    });

    let resp = client
        .post(&url)
        .header("x-goog-api-key", &auth.api_key)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let val: Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("Gemini API error ({status}): {val}");
    }

    let mut texts = Vec::new();
    let mut calls = Vec::new();
    if let Some(parts) = val["candidates"][0]["content"]["parts"].as_array() {
        for (idx, p) in parts.iter().enumerate() {
            if let Some(t) = p["text"].as_str() {
                texts.push(t.to_string());
            }
            if let Some(fc) = p.get("functionCall") {
                calls.push(ToolCall {
                    id: format!("gem_{idx}"),
                    name: fc["name"].as_str().unwrap_or("").to_string(),
                    args: fc["args"].clone(),
                });
            }
        }
    }

    let combined = texts.join("\n");
    if !calls.is_empty() {
        Ok(LlmTurn::Tools {
            text: if combined.is_empty() {
                None
            } else {
                Some(combined)
            },
            calls,
        })
    } else {
        Ok(LlmTurn::Text(combined))
    }
}

fn execute_tool(workdir: &Path, mode: ExecMode, name: &str, args: &Value) -> String {
    let is_write = matches!(name, "write_file" | "edit_file");
    let is_exec = name == "run_command";

    if mode == ExecMode::Plan && (is_write || is_exec) {
        ui::print_warn(&format!("blocked `{name}` in plan mode"));
        return "Denied: currently in read-only `plan` mode.".to_string();
    }

    let needs_confirm = match mode {
        ExecMode::Auto | ExecMode::Plan => false,
        ExecMode::AcceptEdits => is_exec,
        ExecMode::Confirm => is_write || is_exec,
    };

    let summary = format!("{name}({args})");
    if needs_confirm {
        if !prompt_confirm(&summary) {
            ui::print_warn(&format!("denied {name}"));
            return "Denied by user.".to_string();
        }
    } else {
        ui::print_info(&format!("tool: {name}"));
    }

    let out = match name {
        "list_files" => tool_list_files(workdir, args["path"].as_str()),
        "read_file" => tool_read_file(workdir, args["path"].as_str().unwrap_or("")),
        "write_file" => tool_write_file(
            workdir,
            args["path"].as_str().unwrap_or(""),
            args["content"].as_str().unwrap_or(""),
        ),
        "edit_file" => tool_edit_file(
            workdir,
            args["path"].as_str().unwrap_or(""),
            args["target"].as_str().unwrap_or(""),
            args["replacement"].as_str().unwrap_or(""),
        ),
        "search_text" => tool_search_text(workdir, args["query"].as_str().unwrap_or("")),
        "run_command" => tool_run_command(workdir, args["command"].as_str().unwrap_or("")),
        other => Err(format!("unknown tool: {other}")),
    };

    match out {
        Ok(s) => truncate_str(&s, MAX_TOOL_OUTPUT_BYTES),
        Err(e) => format!("ERROR: {e}"),
    }
}

fn prompt_confirm(summary: &str) -> bool {
    print!("  ? Allow {summary} [y/N]: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn resolve_scoped_path(workdir: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Err("path must not be empty".to_string());
    }
    let candidate = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        workdir.join(rel)
    };
    let canon_root = workdir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize workdir: {e}"))?;
    let check = if candidate.exists() {
        candidate.canonicalize()
    } else if let Some(parent) = candidate.parent() {
        if parent.exists() {
            parent
                .canonicalize()
                .map(|p| p.join(candidate.file_name().unwrap_or_default()))
        } else {
            Ok(candidate.clone())
        }
    } else {
        Ok(candidate.clone())
    }
    .map_err(|e| e.to_string())?;

    if !check.starts_with(&canon_root) {
        return Err(format!("path escapes workspace: {rel}"));
    }
    Ok(check)
}

fn tool_list_files(workdir: &Path, sub: Option<&str>) -> Result<String, String> {
    let base = match sub {
        Some(p) if !p.is_empty() => resolve_scoped_path(workdir, p)?,
        _ => workdir.to_path_buf(),
    };
    let mut entries = Vec::new();
    walk_dir(&base, &base, 0, &mut entries);
    entries.sort();
    Ok(entries.join("\n"))
}

fn walk_dir(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 4 || out.len() >= 200 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let path = entry.path();
        if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
        if path.is_dir() {
            walk_dir(root, &path, depth + 1, out);
        }
    }
}

fn tool_read_file(workdir: &Path, rel: &str) -> Result<String, String> {
    let path = resolve_scoped_path(workdir, rel)?;
    fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn tool_write_file(workdir: &Path, rel: &str, content: &str) -> Result<String, String> {
    let path = resolve_scoped_path(workdir, rel)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(format!("wrote {} bytes to {rel}", content.len()))
}

fn tool_edit_file(
    workdir: &Path,
    rel: &str,
    target: &str,
    replacement: &str,
) -> Result<String, String> {
    let path = resolve_scoped_path(workdir, rel)?;
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if !content.contains(target) {
        return Err("target string not found in file".to_string());
    }
    let updated = content.replacen(target, replacement, 1);
    fs::write(&path, updated).map_err(|e| e.to_string())?;
    Ok(format!("edited {rel}"))
}

fn tool_search_text(workdir: &Path, query: &str) -> Result<String, String> {
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    let mut files = Vec::new();
    walk_dir(workdir, workdir, 0, &mut files);
    let mut hits = Vec::new();
    for rel in files {
        let p = workdir.join(&rel);
        if !p.is_file() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&p) {
            for (i, line) in text.lines().enumerate() {
                if line.contains(query) {
                    hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    if hits.len() >= 50 {
                        return Ok(hits.join("\n"));
                    }
                }
            }
        }
    }
    if hits.is_empty() {
        Ok("no matches".to_string())
    } else {
        Ok(hits.join("\n"))
    }
}

fn tool_run_command(workdir: &Path, command: &str) -> Result<String, String> {
    if command.trim().is_empty() {
        return Err("command must not be empty".to_string());
    }
    let out = Command::new("sh")
        .args(["-c", command])
        .current_dir(workdir)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(format!(
        "exit={}\nstdout:\n{}\nstderr:\n{}",
        out.status.code().unwrap_or(-1),
        stdout,
        stderr
    ))
}

fn truncate_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scoped_path_blocks_traversal_and_tools_work() {
        let dir = TempDir::new().unwrap();
        assert!(resolve_scoped_path(dir.path(), "../../etc/passwd").is_err());

        let wrote = tool_write_file(dir.path(), "a.txt", "hello world").unwrap();
        assert!(wrote.contains("11 bytes"));
        assert_eq!(tool_read_file(dir.path(), "a.txt").unwrap(), "hello world");

        tool_edit_file(dir.path(), "a.txt", "world", "latch").unwrap();
        assert_eq!(tool_read_file(dir.path(), "a.txt").unwrap(), "hello latch");

        let found = tool_search_text(dir.path(), "latch").unwrap();
        assert!(found.contains("a.txt:1"));
    }
}
