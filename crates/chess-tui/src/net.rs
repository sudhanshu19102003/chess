//! The network side of a peer-to-peer game. It runs on a tokio runtime
//! in the background and talks to the UI loop over channels: the UI sends
//! [`Request`]s, and gets [`NetEvent`]s back. All rules and clock decisions
//! stay in the UI's `Session`; this side only connects and carries bytes.

use std::time::Duration;

use chess_core::Position;
use chess_net::connect::{self, Servers, Stage, Start};
use chess_net::handshake::Offer;
use chess_net::link;
use chess_net::{Path, TimeControl};
use chess_verify::Session;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// What the UI asks of the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Host(TimeControl),
    Join(String),
    /// A signed message for the opponent.
    Send(Vec<u8>),
    /// Stop connecting, or leave the game.
    Hangup,
}

pub enum NetEvent {
    Stage(Stage),
    /// Connected: the session to play in, and how the connection goes.
    Ready { session: Box<Session>, path: Path, rtt: Duration },
    /// Connecting failed.
    Failed(Failure),
    /// During the game.
    Link(link::Event),
}

/// Why connecting failed, as the lobby shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub title: &'static str,
    pub message: String,
    /// The technical reason, shown faintly under the message.
    pub detail: Option<String>,
}

impl From<&connect::Error> for Failure {
    fn from(e: &connect::Error) -> Failure {
        match e {
            // The name resolved, so the player's internet works: the server
            // is down.
            connect::Error::NoServer { resolved: true, error } => Failure {
                title: "SERVER UNAVAILABLE",
                message: "Sorry, the game server isn't answering right now. This is on our side, not \
                          your connection. Offline games still work. Try again later, or use your own \
                          server with --server HOST."
                    .into(),
                detail: Some(error.to_string()),
            },
            // Either the player is offline or the server's name is gone.
            connect::Error::NoServer { resolved: false, error } => Failure {
                title: "CAN'T REACH THE SERVER",
                message: "Sorry, the game server can't be reached. If your internet works, the server \
                          is down on our side: try again later. Offline games still work."
                    .into(),
                detail: Some(error.to_string()),
            },
            e => Failure { title: "COULD NOT CONNECT", message: e.to_string(), detail: None },
        }
    }
}

struct Call {
    task: JoinHandle<()>,
    out: UnboundedSender<Vec<u8>>,
    events: UnboundedReceiver<NetEvent>,
}

impl Drop for Call {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct Net {
    runtime: Runtime,
    servers: Servers,
    call: Option<Call>,
}

impl Net {
    pub fn new(servers: Servers) -> std::io::Result<Net> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
        Ok(Net { runtime, servers, call: None })
    }

    pub fn request(&mut self, request: Request) {
        let start = match request {
            // Colours by coin flip until the lobby offers a choice.
            Request::Host(tc) => Start::Host(Offer { time_control: tc, colour: None }),
            Request::Join(code) => Start::Join(code),
            Request::Send(bytes) => {
                if let Some(call) = &self.call {
                    let _ = call.out.send(bytes);
                }
                return;
            }
            Request::Hangup => {
                self.call = None;
                return;
            }
        };
        let (out, out_rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::unbounded_channel();
        let servers = self.servers.clone();
        let task = self.runtime.spawn(async move {
            let progress = |stage| {
                let _ = events_tx.send(NetEvent::Stage(stage));
            };
            let linked = match connect::connect(&servers, start, progress).await {
                Ok(linked) => linked,
                Err(e) => {
                    let _ = events_tx.send(NetEvent::Failed(Failure::from(&e)));
                    return;
                }
            };
            let e = linked.established;
            let session = Session::new(e.game_id, Position::starting(), e.me, e.key, e.opponent, e.time_control);
            let _ = events_tx.send(NetEvent::Ready { session: Box::new(session), path: linked.path, rtt: linked.rtt });
            let (link_tx, mut link_rx) = mpsc::unbounded_channel();
            let forward = async {
                while let Some(event) = link_rx.recv().await {
                    let _ = events_tx.send(NetEvent::Link(event));
                }
            };
            tokio::join!(link::run(e.channel, out_rx, link_tx), forward);
        });
        self.call = Some(Call { task, out, events });
    }

    /// Everything that has arrived since the last call.
    pub fn poll(&mut self) -> Vec<NetEvent> {
        let mut got = Vec::new();
        if let Some(call) = &mut self.call {
            while let Ok(event) = call.events.try_recv() {
                got.push(event);
            }
        }
        got
    }
}
