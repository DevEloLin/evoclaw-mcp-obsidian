//! End-to-end MCP protocol test: spawn the real binary and drive it over
//! stdio JSON-RPC, exactly as EvoClaw's MCP client would.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use serial_test::serial;

/// A spawned server plus a background reader that funnels stdout lines to a
/// channel, so the test can wait for a specific response id with a timeout.
struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Server {
    fn start(vault: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_evoclaw-mcp-obsidian"))
            .arg("--vault")
            .arg(vault)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server binary");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });

        Server { child, stdin, rx }
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read lines until a JSON-RPC message with the given id arrives.
    fn recv_id(&self, id: i64) -> Value {
        loop {
            let line = self
                .rx
                .recv_timeout(Duration::from_secs(20))
                .expect("timed out waiting for response");
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("id").and_then(Value::as_i64) == Some(id) {
                    return v;
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("k8s.md"),
        "---\ntags: [devops]\n---\n# Kubernetes 部署\n用 kubectl apply 部署，滚动更新用 rollout。\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cook.md"),
        "---\ntags: [cooking]\n---\n# 番茄炒蛋\n先炒蛋再放番茄，加糖提鲜。\n",
    )
    .unwrap();
    dir
}

#[test]
#[serial]
fn initialize_list_and_call_tools() {
    let vault = write_fixture();
    let mut s = Server::start(vault.path());

    s.send(json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2024-11-05","capabilities":{},
                  "clientInfo":{"name":"e2e","version":"0"}}
    }));
    let init = s.recv_id(1);
    assert!(init["result"]["capabilities"]["tools"].is_object());

    s.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    s.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = s.recv_id(2);
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"kb_search"));
    assert!(names.contains(&"kb_read"));

    // Chinese query + tag filter -> exactly the cooking note.
    s.send(json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"kb_search","arguments":{"query":"提鲜","tags":["cooking"]}}
    }));
    let r = s.recv_id(3);
    let hits: Value =
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["path"], "cook.md");

    // Read a full note.
    s.send(json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"kb_read","arguments":{"path":"k8s.md"}}
    }));
    let r = s.recv_id(4);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("kubectl"));

    // Path traversal is rejected.
    s.send(json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"kb_read","arguments":{"path":"../secret.md"}}
    }));
    let r = s.recv_id(5);
    let msg = r["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("escape"), "expected path-escape error, got: {r}");
}
