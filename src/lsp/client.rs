use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{read_message, write_message};

#[derive(Debug, Clone)]
pub struct LspClientConfig {
    pub command: String,
    pub args: Vec<String>,
    pub root: PathBuf,
    pub root_markers: Vec<String>,
    pub language_id: String,
}

#[derive(Debug, Clone)]
pub enum LspClientEvent {
    Started,
    Initialized,
    Message(Value),
    Stderr(String),
    Exited(Option<i32>),
    Error(String),
}

#[derive(Debug)]
pub enum LspClientCommand {
    Send(Value),
    /// A `textDocument/didChange` full-document sync, carried as a `Rope`
    /// instead of a pre-built JSON `Value`. Cloning a `Rope` is O(1)
    /// (structural sharing), so constructing this effect on the UI thread is
    /// cheap; the expensive part — stringifying the whole document and
    /// building the JSON payload — happens here, on this client's dedicated
    /// writer thread, right before the message is written. Sent over the
    /// same channel as `Send`, so per-document version ordering is
    /// preserved: this is a single-consumer FIFO, not a detached spawn per
    /// edit.
    SendDidChange {
        uri: String,
        version: i32,
        text: ropey::Rope,
    },
    Shutdown,
}

pub struct LspClient {
    commands: Sender<LspClientCommand>,
    join: Option<JoinHandle<()>>,
}

impl LspClient {
    pub fn spawn(
        config: LspClientConfig,
        emit: impl Fn(LspClientEvent) + Send + Sync + 'static,
    ) -> Self {
        let (commands, receiver) = mpsc::channel();
        let emit = Arc::new(emit);
        let join = thread::spawn(move || run_client(config, receiver, emit));
        Self {
            commands,
            join: Some(join),
        }
    }

    pub fn send(&self, message: Value) -> Result<(), String> {
        self.commands
            .send(LspClientCommand::Send(message))
            .map_err(|error| error.to_string())
    }

    /// Queues a `textDocument/didChange` full-document sync without
    /// stringifying the buffer or building its JSON payload on the caller's
    /// thread (see [`LspClientCommand::SendDidChange`]).
    pub fn send_did_change(
        &self,
        uri: String,
        version: i32,
        text: ropey::Rope,
    ) -> Result<(), String> {
        self.commands
            .send(LspClientCommand::SendDidChange { uri, version, text })
            .map_err(|error| error.to_string())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        self.commands
            .send(LspClientCommand::Shutdown)
            .map_err(|error| error.to_string())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.commands.send(LspClientCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

enum Incoming {
    Message(Value),
    Eof,
    Error(String),
}

fn run_client(
    mut config: LspClientConfig,
    commands: Receiver<LspClientCommand>,
    emit: Arc<impl Fn(LspClientEvent) + Send + Sync + 'static>,
) {
    config.root = detect_root(&config.root, &config.root_markers);
    let mut child = match Command::new(&config.command)
        .args(&config.args)
        .current_dir(&config.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            emit(LspClientEvent::Error(format!(
                "cannot start {}: {error}",
                config.command
            )));
            return;
        }
    };
    let Some(mut writer) = child.stdin.take() else {
        emit(LspClientEvent::Error("LSP stdin unavailable".to_owned()));
        terminate(&mut child);
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        emit(LspClientEvent::Error("LSP stdout unavailable".to_owned()));
        terminate(&mut child);
        return;
    };
    let (incoming_sender, incoming) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_message(&mut reader) {
                Ok(Some(message)) => {
                    if incoming_sender.send(Incoming::Message(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = incoming_sender.send(Incoming::Eof);
                    break;
                }
                Err(error) => {
                    let _ = incoming_sender.send(Incoming::Error(error.to_string()));
                    break;
                }
            }
        }
    });
    if let Some(stderr) = child.stderr.take() {
        let stderr_emit = emit.clone();
        thread::spawn(move || {
            let mut stderr = BufReader::new(stderr);
            let mut buffer = String::new();
            if stderr.read_to_string(&mut buffer).is_ok() && !buffer.trim().is_empty() {
                stderr_emit(LspClientEvent::Stderr(buffer));
            }
        });
    }

    emit(LspClientEvent::Started);
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": path_uri(&config.root),
            "capabilities": {
                "textDocument": {
                    "hover": {},
                    "definition": {},
                    "completion": {"completionItem": {"snippetSupport": false}},
                    "publishDiagnostics": {}
                }
            },
            "clientInfo": {"name": "Mica", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    if let Err(error) = write_message(&mut writer, &initialize) {
        emit(LspClientEvent::Error(error.to_string()));
        terminate(&mut child);
        let _ = reader.join();
        return;
    }

    let mut initialized = false;
    let mut shutting_down = false;
    let mut shutdown_started = None;
    loop {
        while let Ok(message) = incoming.try_recv() {
            match message {
                Incoming::Message(message) => {
                    if message.get("id") == Some(&json!(1)) && !initialized {
                        initialized = true;
                        let _ = write_message(
                            &mut writer,
                            &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
                        );
                        emit(LspClientEvent::Initialized);
                    } else if message.get("method").is_some() && message.get("id").is_some() {
                        let _ = respond_unknown_request(&mut writer, &message);
                    }
                    emit(LspClientEvent::Message(message));
                }
                Incoming::Eof => {
                    shutting_down = true;
                    shutdown_started.get_or_insert_with(Instant::now);
                }
                Incoming::Error(error) => {
                    emit(LspClientEvent::Error(error));
                    shutting_down = true;
                    shutdown_started.get_or_insert_with(Instant::now);
                }
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            emit(LspClientEvent::Exited(status.code()));
            break;
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(LspClientCommand::Send(message)) if !shutting_down => {
                if let Err(error) = write_message(&mut writer, &message) {
                    emit(LspClientEvent::Error(error.to_string()));
                    shutting_down = true;
                    shutdown_started = Some(Instant::now());
                }
            }
            Ok(LspClientCommand::SendDidChange { uri, version, text }) if !shutting_down => {
                // Stringify the document and build the JSON payload here, on
                // this dedicated writer thread, instead of on the UI thread
                // that queued it.
                let message = json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {"uri": uri, "version": version},
                        "contentChanges": [{"text": text.to_string()}]
                    }
                });
                if let Err(error) = write_message(&mut writer, &message) {
                    emit(LspClientEvent::Error(error.to_string()));
                    shutting_down = true;
                    shutdown_started = Some(Instant::now());
                }
            }
            Ok(LspClientCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !shutting_down {
                    let _ = write_message(
                        &mut writer,
                        &json!({"jsonrpc":"2.0","id":u64::MAX,"method":"shutdown","params":null}),
                    );
                    let _ = write_message(
                        &mut writer,
                        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
                    );
                    shutting_down = true;
                    shutdown_started = Some(Instant::now());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) | Ok(_) => {}
        }
        if shutting_down
            && shutdown_started.is_some_and(|started| started.elapsed() >= Duration::from_secs(1))
        {
            terminate(&mut child);
        }
    }
    drop(writer);
    let _ = reader.join();
}

fn respond_unknown_request(
    writer: &mut impl std::io::Write,
    message: &Value,
) -> Result<(), String> {
    let response = json!({
        "jsonrpc": "2.0",
        "id": message.get("id").cloned().unwrap_or(Value::Null),
        "error": {"code": -32601, "message": "Method not implemented by Mica"}
    });
    write_message(writer, &response).map_err(|error| error.to_string())
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn path_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy().replace(' ', "%20"))
}

fn detect_root(workspace: &std::path::Path, markers: &[String]) -> PathBuf {
    for directory in workspace.ancestors() {
        if markers.iter().any(|marker| directory.join(marker).exists()) {
            return directory.to_path_buf();
        }
    }
    workspace.to_path_buf()
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, Mutex};

    use super::*;

    const MOCK_SERVER: &str = r#"
import json, sys
inp = sys.stdin.buffer
out = sys.stdout.buffer
def read():
    length = None
    while True:
        line = inp.readline()
        if not line: return None
        if line in (b'\r\n', b'\n'): break
        name, value = line.decode().split(':', 1)
        if name.lower() == 'content-length': length = int(value.strip())
    return json.loads(inp.read(length))
def write(value):
    body = json.dumps(value, separators=(',', ':')).encode()
    out.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    out.flush()
initialize = read()
write({'jsonrpc':'2.0','id':initialize['id'],'result':{'capabilities':{}}})
read()
write({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':'file:///tmp/mock.rs','diagnostics':[]}})
while True:
    message = read()
    if message is None: break
    method = message.get('method')
    if method == 'textDocument/hover':
        write({'jsonrpc':'2.0','id':message['id'],'result':{'contents':'hover'}})
    elif method == 'textDocument/definition':
        write({'jsonrpc':'2.0','id':message['id'],'result':{'uri':'file:///tmp/mock.rs','range':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}}}})
    elif method == 'textDocument/completion':
        write({'jsonrpc':'2.0','id':message['id'],'result':[{'label':'item'}]})
    elif method == 'shutdown':
        write({'jsonrpc':'2.0','id':message['id'],'result':None})
    elif method == 'exit':
        break
"#;

    #[test]
    fn mock_server_completes_lifecycle_diagnostics_and_requests() {
        let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let sink = events.clone();
        let client = LspClient::spawn(
            LspClientConfig {
                command: "python3".to_owned(),
                args: vec!["-c".to_owned(), MOCK_SERVER.to_owned()],
                root: std::env::temp_dir(),
                root_markers: Vec::new(),
                language_id: "rust".to_owned(),
            },
            move |event| {
                let (events, ready) = &*sink;
                events.lock().unwrap().push(event);
                ready.notify_all();
            },
        );
        let (recorded_events, ready) = &*events;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut recorded = recorded_events.lock().unwrap();
        while !recorded
            .iter()
            .any(|event| matches!(event, LspClientEvent::Initialized))
            && Instant::now() < deadline
        {
            recorded = ready
                .wait_timeout(recorded, deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .0;
        }
        assert!(
            recorded
                .iter()
                .any(|event| matches!(event, LspClientEvent::Initialized))
        );
        drop(recorded);

        for (id, method) in [
            (2, "textDocument/hover"),
            (3, "textDocument/definition"),
            (4, "textDocument/completion"),
        ] {
            client
                .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}))
                .unwrap();
        }
        let mut recorded = recorded_events.lock().unwrap();
        while recorded
            .iter()
            .filter(|event| matches!(event, LspClientEvent::Message(message) if message.get("id").is_some()))
            .count()
            < 4
            && Instant::now() < deadline
        {
            recorded = ready
                .wait_timeout(recorded, deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .0;
        }
        assert!(recorded.iter().any(|event| matches!(
            event,
            LspClientEvent::Message(message)
                if message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
        )));
        for id in 2..=4 {
            assert!(recorded.iter().any(|event| matches!(
                event,
                LspClientEvent::Message(message) if message.get("id") == Some(&json!(id))
            )));
        }
        drop(recorded);
        drop(client);
        assert!(
            events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, LspClientEvent::Exited(Some(0))))
        );
    }
}
