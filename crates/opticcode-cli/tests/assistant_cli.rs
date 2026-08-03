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
