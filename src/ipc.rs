//! The window talks to the background blocker over a local TCP connection, one JSON line per
//! request and response. The port and a random token are in service.json, readable only by
//! this user, and every request must carry the token.

use crate::engine::{Edit, LockRequest, PublicBlock, Settings};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    Ping,
    GetState,
    CreateBlock { name: String },
    UpdateBlock { id: String, edit: Edit },
    DeleteBlock { id: String },
    StartBlock { id: String, ends_at: i64 },
    SetLock { id: String, lock: LockRequest },
    Challenge { id: String },
    StopBlock { id: String, answer: String },
    UpdateSettings { start_at_login: bool },
    SetupHosts,
    RunningApps,
    Quit,
    /// A second settings window was opened: bring the existing one to the front instead.
    ShowWindow,
    /// Tests only: positions of cover windows.
    Covers,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    #[serde(flatten)]
    req: Request,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsStatus {
    pub path: String,
    pub writable: bool,
    pub needs_setup: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicState {
    pub now: i64,
    pub blocks: Vec<PublicBlock>,
    pub settings: Settings,
    pub closed_today: u32,
    pub hosts: HostsStatus,
    pub platform: String,
    pub cover_supported: bool,
    pub version: String,
    /// Goes up each time the settings window should come to the front.
    pub show_requests: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Challenge {
    None,
    Locked { until: Option<i64> },
    Password,
    Random { text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverInfo {
    pub target: i64,
    pub pid: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub visible: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<PublicState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge: Option<Challenge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covers: Option<Vec<CoverInfo>>,
}

impl Response {
    pub fn ok() -> Self {
        Response { ok: true, ..Default::default() }
    }
    pub fn err(e: impl Into<String>) -> Self {
        Response { ok: false, error: Some(e.into()), ..Default::default() }
    }
}

/// Contents of service.json.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

pub fn endpoint_path() -> std::path::PathBuf {
    crate::util::data_dir().join("service.json")
}

pub fn read_endpoint() -> Option<Endpoint> {
    serde_json::from_str(&std::fs::read_to_string(endpoint_path()).ok()?).ok()
}

pub struct Client {
    ep: Endpoint,
}

impl Client {
    /// Connects to the running background blocker, if there is one.
    pub fn connect() -> Option<Client> {
        let c = Client { ep: read_endpoint()? };
        c.send(&Request::Ping).ok().filter(|r| r.ok).map(|_| c)
    }

    pub fn send(&self, req: &Request) -> Result<Response, String> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], self.ep.port));
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
        // Setting up website blocking waits for the user at the password prompt.
        let wait = if matches!(req, Request::SetupHosts) { 300 } else { 15 };
        s.set_read_timeout(Some(Duration::from_secs(wait))).ok();
        let mut line = serde_json::to_string(&Envelope { token: self.ep.token.clone(), req: req.clone() }).map_err(|e| e.to_string())?;
        line.push('\n');
        s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(s);
        let mut resp = String::new();
        reader.read_line(&mut resp).map_err(|e| e.to_string())?;
        serde_json::from_str(&resp).map_err(|e| format!("bad response: {e}"))
    }
}

/// Serves requests: `handle` is called for each one, from a thread per connection.
pub fn serve(handle: impl Fn(Request) -> Response + Send + Sync + 'static) -> std::io::Result<Endpoint> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let mut tok = [0u8; 24];
    let _ = getrandom::fill(&mut tok);
    let ep = Endpoint { port: listener.local_addr()?.port(), token: tok.iter().map(|b| format!("{b:02x}")).collect(), pid: std::process::id() };
    let path = endpoint_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string(&ep).unwrap())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let token = ep.token.clone();
    let handle = std::sync::Arc::new(handle);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handle = handle.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                });
                let mut writer = stream;
                let mut line = String::new();
                while reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    let resp = match serde_json::from_str::<Envelope>(&line) {
                        Ok(env) if env.token == token => handle(env.req),
                        Ok(_) => Response::err("bad token"),
                        Err(e) => Response::err(format!("bad request: {e}")),
                    };
                    let mut out = serde_json::to_string(&resp).unwrap();
                    out.push('\n');
                    if writer.write_all(out.as_bytes()).is_err() {
                        break;
                    }
                    line.clear();
                }
            });
        }
    });
    Ok(ep)
}
