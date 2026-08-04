use std::io::{self, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use opticcode_core::{
    chat_event_channel, chat_setup_failure_event, execute_chat, validate_chat_request_id,
    CancellationToken, ChatCommand, ChatControlKind, ChatControlMessage, ChatExecutionStatus,
    ChatProtocolEvent, ChatProtocolSession, ChatRequest, ChatRuntimeOptions,
    CHAT_CONTROL_PROTOCOL_ID, CHAT_PROTOCOL_ID, CHAT_PROTOCOL_SCHEMA_VERSION,
    DEFAULT_CHAT_EVENT_CAPACITY, MAX_CHAT_REQUEST_BYTES,
};
use serde_json::Value;

use crate::create_opticcode;

const MAX_CONTROL_BYTES: usize = 512;

#[derive(Debug, Parser)]
#[command(name = "opticcode chat")]
#[command(about = "Run the versioned OpticCode chat protocol over stdin and NDJSON stdout.")]
pub struct ChatArgs {
    #[arg(long)]
    protocol_jsonl: bool,
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,
    #[arg(long, default_value = "15m")]
    keep_alive: String,
    #[arg(long, default_value = "data/index")]
    rag_index: PathBuf,
    #[arg(long, default_value_t = 120_000)]
    http_timeout_ms: u64,
}

pub fn parse_isolated_chat() -> Option<ChatArgs> {
    let mut args = std::env::args_os();
    args.next()?;
    let first = args.next()?;
    let mut parser_args = vec![std::ffi::OsString::from("opticcode chat")];
    if first == std::ffi::OsStr::new("chat") {
        parser_args.extend(args);
    } else if first == std::ffi::OsStr::new("help")
        && args.next().as_deref() == Some(std::ffi::OsStr::new("chat"))
    {
        parser_args.push(std::ffi::OsString::from("--help"));
        parser_args.extend(args);
    } else {
        return None;
    }
    Some(ChatArgs::parse_from(parser_args))
}

pub async fn run_chat(args: ChatArgs) -> Result<i32> {
    if !args.protocol_jsonl {
        bail!("chat currently requires --protocol-jsonl");
    }
    if args.http_timeout_ms == 0 {
        bail!("chat provider timeout must be greater than zero");
    }

    let raw = match read_bounded_line(MAX_CHAT_REQUEST_BYTES) {
        Ok(raw) => raw,
        Err(error) => {
            emit_setup_failure(
                "chat-invalid-request",
                "invalid_request",
                &error.to_string(),
            )?;
            return Ok(2);
        }
    };
    let fallback_id = extract_request_id(&raw).unwrap_or_else(generated_invalid_request_id);
    let request = match serde_json::from_slice::<ChatRequest>(&raw) {
        Ok(request) => request,
        Err(error) => {
            emit_setup_failure(
                &fallback_id,
                "invalid_request",
                &format!("chat request is not valid schema v1 JSON: {error}"),
            )?;
            return Ok(2);
        }
    };
    if request.request_id != fallback_id {
        emit_setup_failure(
            &fallback_id,
            "request_mismatch",
            "decoded chat request ID changed during validation",
        )?;
        return Ok(2);
    }

    let app = if matches!(
        request.command,
        ChatCommand::Ask | ChatCommand::Plan | ChatCommand::Fix
    ) {
        match create_opticcode(
            &args.ollama_url,
            &request.model,
            &args.keep_alive,
            args.http_timeout_ms,
        ) {
            Ok(app) => Some(app),
            Err(error) => {
                emit_setup_failure(
                    &request.request_id,
                    "provider_setup_failed",
                    &format!("local provider setup failed: {error:#}"),
                )?;
                return Ok(2);
            }
        }
    } else {
        None
    };

    let (events, mut receiver) = chat_event_channel(DEFAULT_CHAT_EVENT_CAPACITY)?;
    let cancellation = CancellationToken::new();
    let session = ChatProtocolSession {
        request_id: request.request_id.clone(),
        events,
        cancellation: cancellation.clone(),
    };
    watch_control_stdin(request.request_id.clone(), cancellation.clone());
    let signal_cancellation = cancellation.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancellation.cancel();
        }
    });
    tokio::task::yield_now().await;

    let execution = execute_chat(
        app.as_ref(),
        request,
        session,
        ChatRuntimeOptions {
            rag_index: args.rag_index,
            verify_model: true,
            policy_state_root: None,
            proposal_state_root: None,
        },
    );
    tokio::pin!(execution);
    let mut execution_result = None;
    let mut events_open = true;
    let mut next_sequence = 0u64;
    let mut terminal_count = 0usize;
    while execution_result.is_none() || events_open {
        tokio::select! {
            event = receiver.recv(), if events_open => {
                match event {
                    Some(event) => {
                        validate_and_render_event(&event, &mut next_sequence, &mut terminal_count)?;
                    }
                    None => events_open = false,
                }
            }
            result = &mut execution, if execution_result.is_none() => {
                execution_result = Some(result);
            }
        }
    }
    signal_task.abort();
    if terminal_count != 1 {
        bail!("chat protocol expected exactly one terminal event, received {terminal_count}");
    }
    let report = execution_result.context("chat protocol execution ended without a result")??;
    Ok(match report.status {
        ChatExecutionStatus::Completed => 0,
        ChatExecutionStatus::Failed | ChatExecutionStatus::Cancelled => 2,
    })
}

fn read_bounded_line(limit: usize) -> Result<Vec<u8>> {
    let mut stdin = io::stdin().lock();
    let mut line = Vec::with_capacity(8 * 1024);
    let mut byte = [0u8; 1];
    loop {
        match stdin.read(&mut byte) {
            Ok(0) if line.is_empty() => bail!("chat stdin ended before the initial request"),
            Ok(0) => bail!("chat initial request must end with a newline"),
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if line.len() >= limit => {
                bail!("chat initial request exceeds the {limit}-byte limit")
            }
            Ok(_) => line.push(byte[0]),
            Err(error) => return Err(error).context("failed to read chat stdin"),
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if line.is_empty() {
        bail!("chat initial request line must not be empty");
    }
    std::str::from_utf8(&line).context("chat initial request is not valid UTF-8")?;
    Ok(line)
}

fn extract_request_id(raw: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(raw).ok()?;
    let request_id = value.get("request_id")?.as_str()?.to_string();
    validate_chat_request_id(&request_id).ok()?;
    Some(request_id)
}

fn generated_invalid_request_id() -> String {
    format!("chat-invalid-{}", std::process::id())
}

fn emit_setup_failure(request_id: &str, code: &str, message: &str) -> Result<()> {
    let event = chat_setup_failure_event(request_id, code, bounded_message(message));
    println!("{}", serde_json::to_string(&event)?);
    io::stdout().flush()?;
    Ok(())
}

fn watch_control_stdin(request_id: String, cancellation: CancellationToken) {
    std::thread::spawn(move || loop {
        let line = match read_bounded_line(MAX_CONTROL_BYTES) {
            Ok(line) => line,
            Err(_) => return,
        };
        if line == b"cancel" {
            cancellation.cancel();
            return;
        }
        let Ok(control) = serde_json::from_slice::<ChatControlMessage>(&line) else {
            cancellation.cancel();
            return;
        };
        if control.schema_version != CHAT_PROTOCOL_SCHEMA_VERSION
            || control.protocol != CHAT_CONTROL_PROTOCOL_ID
            || control.request_id != request_id
        {
            cancellation.cancel();
            return;
        }
        if control.kind == ChatControlKind::Cancel {
            cancellation.cancel();
            return;
        }
    });
}

fn validate_and_render_event(
    event: &ChatProtocolEvent,
    next_sequence: &mut u64,
    terminal_count: &mut usize,
) -> Result<()> {
    if event.schema_version != CHAT_PROTOCOL_SCHEMA_VERSION || event.protocol != CHAT_PROTOCOL_ID {
        bail!("chat protocol identity or schema version mismatch");
    }
    if event.sequence != *next_sequence {
        bail!(
            "chat protocol sequence mismatch: expected {}, received {}",
            *next_sequence,
            event.sequence
        );
    }
    if *terminal_count > 0 {
        bail!("chat protocol emitted an event after its terminal event");
    }
    println!("{}", serde_json::to_string(event)?);
    io::stdout().flush()?;
    *next_sequence = next_sequence.saturating_add(1);
    if event.is_terminal() {
        *terminal_count += 1;
    }
    Ok(())
}

fn bounded_message(value: &str) -> String {
    let mut message = value.chars().take(8 * 1024).collect::<String>();
    if value.chars().count() > 8 * 1024 {
        message.push_str("...[truncated]");
    }
    message
}
