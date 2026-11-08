use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use serde_json::{json, Value};

use crate::effects::Outcome;
use crate::game;
use crate::log;

static OUTBOUND_SOCKET: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();
static SESSION_SECRET: OnceLock<Mutex<String>> = OnceLock::new();

pub struct Io {
    pub commands: Receiver<Command>,
    pub outbound: Sender<String>,
}

pub enum Command {
    Trigger {
        request_id: String,
        handler: String,
        args: Value,
    },
}

/// Counts connections to the manager, which needs one hello at the start of each.
static CONNECTIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn connection_count() -> u32 {
    CONNECTIONS.load(std::sync::atomic::Ordering::Acquire)
}

pub fn current_secret() -> String {
    SESSION_SECRET
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .expect("secret")
        .clone()
}

pub fn connect_and_serve() -> Result<Io, String> {
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (outbound_tx, outbound_rx) = crossbeam_channel::unbounded::<String>();
    OUTBOUND_SOCKET.get_or_init(|| Mutex::new(None));
    SESSION_SECRET.get_or_init(|| Mutex::new(String::new()));
    std::thread::spawn(move || write_loop(outbound_rx));
    std::thread::spawn(move || reconnect_loop(command_tx));
    Ok(Io {
        commands: command_rx,
        outbound: outbound_tx,
    })
}

fn write_loop(outbound: Receiver<String>) {
    while let Ok(line) = outbound.recv() {
        let mut guard = OUTBOUND_SOCKET
            .get()
            .expect("socket")
            .lock()
            .expect("socket mutex");
        let failed = if let Some(stream) = guard.as_mut() {
            writeln!(stream, "{line}").is_err()
        } else {
            false
        };
        if failed {
            *guard = None;
        }
    }
}

fn reconnect_loop(commands: Sender<Command>) {
    loop {
        match read_session() {
            Ok(session) => {
                *SESSION_SECRET
                    .get()
                    .expect("secret")
                    .lock()
                    .expect("secret") = session.secret;
                match TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], session.port)),
                    Duration::from_secs(2),
                ) {
                    Ok(stream) => {
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        if let Ok(writer) = stream.try_clone() {
                            *OUTBOUND_SOCKET.get().expect("socket").lock().expect("socket") =
                                Some(writer);
                            CONNECTIONS.fetch_add(1, std::sync::atomic::Ordering::Release);
                        }
                        if let Err(error) = read_loop(stream, &commands) {
                            log::log(format!("connection closed: {error}"));
                        }
                        *OUTBOUND_SOCKET.get().expect("socket").lock().expect("socket") = None;
                    }
                    Err(error) => log::log(format!("waiting for the manager: {error}")),
                }
            }
            Err(error) => log::log(error),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn read_loop(stream: TcpStream, commands: &Sender<Command>) -> Result<(), String> {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut || error.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if value.get("type").and_then(Value::as_str) != Some("trigger") {
            continue;
        }
        let request_id = value
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let handler = value
            .get("handler")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let args = value.get("args").cloned().unwrap_or(json!({}));
        if commands
            .send(Command::Trigger {
                request_id,
                handler,
                args,
            })
            .is_err()
        {
            return Err("command channel closed".to_string());
        }
    }
    Err("manager closed the connection".to_string())
}

struct SessionFile {
    secret: String,
    port: u16,
}

fn read_session() -> Result<SessionFile, String> {
    let path = log::app_dir()?.join("session.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("session file is missing ({})", path.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let secret = value
        .get("secret")
        .and_then(Value::as_str)
        .ok_or("session file has no secret")?
        .to_string();
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .ok_or("session file has no port")? as u16;
    Ok(SessionFile { secret, port })
}

pub fn hello_line() -> String {
    let snapshot = game::snapshot();
    json!({
        "type": "hello",
        "protocol": 1,
        "secret": current_secret(),
        "exeMatch": snapshot.exe_match,
        "sessionKnown": snapshot.session_known,
        "offline": snapshot.offline,
        "inGame": snapshot.in_game,
        "characterName": snapshot.character_name,
        "handlers": crate::effects::schemas(),
    })
    .to_string()
}

pub fn state_line() -> String {
    let snapshot = game::snapshot();
    json!({
        "type": "state",
        "exeMatch": snapshot.exe_match,
        "sessionKnown": snapshot.session_known,
        "offline": snapshot.offline,
        "inGame": snapshot.in_game,
        "characterName": snapshot.character_name,
    })
    .to_string()
}

pub fn result_line(request_id: &str, outcome: &Outcome) -> String {
    let (status, message) = match outcome {
        Outcome::Success(message) => ("success", message.as_str()),
        Outcome::Failed(message) => ("failed", message.as_str()),
        Outcome::Unavailable(message) => ("unavailable", message.as_str()),
        Outcome::Retry(message) => ("retry", message.as_str()),
    };
    json!({
        "type": "result",
        "requestId": request_id,
        "status": status,
        "message": message,
    })
    .to_string()
}
