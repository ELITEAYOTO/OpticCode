use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_opticcode"))
}

#[test]
fn compare_ask_emits_one_pure_json_envelope_without_contacting_ollama() {
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "ask",
            "Locate dev.opticcode.util.Helpers#ping().",
            "--path",
            "benchmarks/java-index-mini",
            "--profile",
            "none",
            "--no-memory",
            "--no-rag",
            "--context-mode",
            "compare",
            "--ollama-url",
            "http://127.0.0.1:9",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "ask");
    assert_eq!(value["requested_context_mode"], "compare");
    assert_eq!(value["runs"].as_array().unwrap().len(), 2);
    assert!(value["runs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|run| run["generated"] == false));
}

#[test]
fn non_local_ollama_url_is_a_structured_json_error() {
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "plan",
            "Find plugin.yml.",
            "--path",
            "benchmarks/java-index-mini",
            "--no-rag",
            "--ollama-url",
            "https://example.com",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["success"], false);
    assert_eq!(value["errors"][0]["code"], "command_rejected");
    assert!(value["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("non-local"));
}

#[test]
fn legacy_metrics_json_output_remains_compatible() {
    let url = spawn_mock_ollama();
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "ask",
            "Locate Helpers#ping().",
            "--path",
            "benchmarks/java-index-mini",
            "--profile",
            "none",
            "--no-memory",
            "--no-rag",
            "--ollama-url",
            &url,
            "--max-tokens",
            "16",
            "--metrics-json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "mock response"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("=== metrics_json ==="));
    assert!(stderr.contains("\"command\": \"ask\""));
    assert!(stderr.contains("\"prompt_eval_count\": 20"));
}

#[test]
fn protocol_jsonl_streams_ordered_versioned_events() {
    let url = spawn_mock_ollama_streaming();
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "ask",
            "Locate Helpers#ping().",
            "--path",
            "benchmarks/java-index-mini",
            "--profile",
            "none",
            "--no-memory",
            "--no-rag",
            "--ollama-url",
            &url,
            "--max-tokens",
            "16",
            "--protocol-jsonl",
            "--request-id",
            "cli-stream-1",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(events.len() >= 6);
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["protocol"], "opticcode.assistant");
        assert_eq!(event["request_id"], "cli-stream-1");
        assert_eq!(event["sequence"], sequence as u64);
    }
    assert_eq!(events.first().unwrap()["type"], "started");
    assert_eq!(events[1]["type"], "context_prepared");
    assert_eq!(events.last().unwrap()["type"], "completed");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event["type"].as_str(),
                Some("completed" | "failed" | "cancelled")
            ))
            .count(),
        1
    );

    let provider_events = events
        .iter()
        .filter(|event| event["type"] == "provider_event")
        .map(|event| &event["event"])
        .collect::<Vec<_>>();
    assert_eq!(provider_events.first().unwrap()["type"], "started");
    assert_eq!(provider_events.last().unwrap()["type"], "completed");
    assert!(provider_events.iter().enumerate().all(|(sequence, event)| {
        event["schema_version"] == 1
            && event["protocol"] == "opticcode.llm"
            && event["sequence"] == sequence as u64
    }));
    let reconstructed = provider_events
        .iter()
        .filter(|event| event["type"] == "delta")
        .filter_map(|event| event["text"].as_str())
        .collect::<String>();
    assert_eq!(reconstructed, "mock response");
}

#[test]
fn human_stream_prints_each_delta_once() {
    let url = spawn_mock_ollama_streaming();
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "plan",
            "Locate Helpers#ping().",
            "--path",
            "benchmarks/java-index-mini",
            "--profile",
            "none",
            "--no-memory",
            "--no-rag",
            "--ollama-url",
            &url,
            "--max-tokens",
            "16",
            "--stream",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "mock response\n");
}

#[test]
fn protocol_setup_failures_are_single_terminal_json_lines() {
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "plan",
            "Locate plugin.yml.",
            "--path",
            "benchmarks/java-index-mini",
            "--no-rag",
            "--ollama-url",
            "https://example.com",
            "--protocol-jsonl",
            "--request-id",
            "setup-failure-1",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let lines = String::from_utf8(output.stdout).unwrap();
    let lines = lines.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(event["protocol"], "opticcode.assistant");
    assert_eq!(event["request_id"], "setup-failure-1");
    assert_eq!(event["sequence"], 0);
    assert_eq!(event["type"], "failed");
    assert_eq!(event["errors"][0]["code"], "command_rejected");
}

#[test]
fn compare_protocol_completes_without_provider_events_or_network() {
    let output = Command::new(binary())
        .current_dir(workspace())
        .args([
            "ask",
            "Locate dev.opticcode.util.Helpers#ping().",
            "--path",
            "benchmarks/java-index-mini",
            "--profile",
            "none",
            "--no-memory",
            "--no-rag",
            "--context-mode",
            "compare",
            "--ollama-url",
            "http://127.0.0.1:9",
            "--protocol-jsonl",
            "--request-id",
            "compare-1",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["type"], "started");
    assert_eq!(events[1]["type"], "context_prepared");
    assert_eq!(events[2]["type"], "completed");
    assert!(!events.iter().any(|event| event["type"] == "provider_event"));
}

#[test]
fn global_and_isolated_help_commands_do_not_overflow_the_windows_stack() {
    for arguments in [
        vec!["--help"],
        vec!["ask", "--help"],
        vec!["plan", "--help"],
    ] {
        let output = Command::new(binary())
            .current_dir(workspace())
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
}

fn spawn_mock_ollama() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for body in [
            r#"{"models":[{"name":"qwen2.5-coder:14b","model":"qwen2.5-coder:14b","size":1}]}"#,
            r#"{"response":"mock response","done":true,"total_duration":1000000,"load_duration":1000,"prompt_eval_count":20,"prompt_eval_duration":2000,"eval_count":5,"eval_duration":3000}"#,
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            read_http_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{address}")
}

fn spawn_mock_ollama_streaming() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut tags_stream, _) = listener.accept().unwrap();
        tags_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        read_http_request(&mut tags_stream);
        let tags =
            r#"{"models":[{"name":"qwen2.5-coder:14b","model":"qwen2.5-coder:14b","size":1}]}"#;
        write_json_response(&mut tags_stream, tags);

        let (mut generation_stream, _) = listener.accept().unwrap();
        generation_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        read_http_request(&mut generation_stream);
        let body = concat!(
            "{\"response\":\"mock \" ,\"done\":false}\n",
            "{\"response\":\"response\",\"done\":false}\n",
            "{\"response\":\"\",\"done\":true,\"done_reason\":\"stop\",",
            "\"total_duration\":1000000,\"load_duration\":1000,",
            "\"prompt_eval_count\":20,\"prompt_eval_duration\":2000,",
            "\"eval_count\":5,\"eval_duration\":3000}\n"
        );
        write_chunked_response(&mut generation_stream, body);
    });
    format!("http://{address}")
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn write_chunked_response(stream: &mut std::net::TcpStream, body: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    for piece in body.as_bytes().chunks(11) {
        stream
            .write_all(format!("{:X}\r\n", piece.len()).as_bytes())
            .unwrap();
        stream.write_all(piece).unwrap();
        stream.write_all(b"\r\n").unwrap();
        stream.flush().unwrap();
    }
    stream.write_all(b"0\r\n\r\n").unwrap();
}

fn read_http_request(stream: &mut std::net::TcpStream) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let header_end = header_end + 4;
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length: ")
                    .or_else(|| line.strip_prefix("Content-Length: "))
            })
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if bytes.len() >= header_end + content_length {
            return;
        }
    }
}
