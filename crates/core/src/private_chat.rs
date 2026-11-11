mod lan;

use anyhow::{Context, Result, bail, ensure};
use quinn::{Connection, ConnectionError, RecvStream, SendStream};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, sleep_until, timeout},
};
use uuid::Uuid;

const DISCOVERY_TIME: Duration = Duration::from_secs(30);
const VERIFICATION_TIME: Duration = Duration::from_secs(90);
const SESSION_IDLE: Duration = Duration::from_secs(15 * 60);
const MAX_TEXT_BYTES: usize = 2048;
const HISTORY_LIMIT: usize = 100;
const END: u32 = 1;
const DECLINED: u32 = 2;
const FAILED: u32 = 3;

#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Status {
    Discovering,
    Connecting,
    Verify { code: String },
    WaitingForPeer { code: String },
    Connected,
    Ended,
    Failed { reason: String },
}

#[derive(Clone, Serialize)]
struct Message {
    seq: u64,
    mine: bool,
    text: String,
}

#[derive(Serialize)]
pub struct PrivateView {
    #[serde(flatten)]
    status: Status,
    messages: Vec<Message>,
}

struct State {
    status: Status,
    messages: VecDeque<Message>,
    seq: u64,
    last_text: Instant,
}

type Shared = Arc<Mutex<State>>;

impl State {
    fn record(&mut self, mine: bool, text: String) {
        self.seq += 1;
        self.last_text = Instant::now();
        if self.messages.len() == HISTORY_LIMIT {
            self.messages.pop_front();
        }
        self.messages.push_back(Message {
            seq: self.seq,
            mine,
            text,
        });
    }
}

pub struct PrivateChat {
    state: Shared,
    verification: Option<oneshot::Sender<bool>>,
    outgoing: mpsc::Sender<String>,
    cancel: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl PrivateChat {
    pub fn start(server: &str, own: Uuid, peer: Uuid) -> Self {
        let host = own < peer;
        let (first, second) = if host { (own, peer) } else { (peer, own) };
        let tag = Sha256::new()
            .chain_update(b"zibblu-private-v1\0")
            .chain_update(first.as_bytes())
            .chain_update(second.as_bytes())
            .chain_update(server)
            .finalize()
            .into();
        let state = Arc::new(Mutex::new(State {
            status: Status::Discovering,
            messages: VecDeque::new(),
            seq: 0,
            last_text: Instant::now(),
        }));
        let (verification, decision) = oneshot::channel();
        let (outgoing, messages) = mpsc::channel(8);
        let (cancel, canceled) = oneshot::channel();
        let shared = state.clone();
        let task = tokio::spawn(async move {
            let status = tokio::select! {
                biased;
                _ = canceled => Status::Ended,
                status = run(host, tag, &shared, decision, messages) => status,
            };
            let mut state = shared.lock().expect("private chat state");
            state.messages.clear();
            state.status = status;
        });
        Self {
            state,
            verification: Some(verification),
            outgoing,
            cancel,
            task,
        }
    }

    pub fn view(&self, after: u64) -> PrivateView {
        let state = self.state.lock().expect("private chat state");
        PrivateView {
            status: state.status.clone(),
            messages: state
                .messages
                .iter()
                .filter(|m| m.seq > after)
                .cloned()
                .collect(),
        }
    }

    pub fn verify(&mut self, matches: bool) -> Result<()> {
        let state = self.state.lock().expect("private chat state");
        ensure!(
            matches!(state.status, Status::Verify { .. }),
            "There is no private connection to verify."
        );
        self.verification
            .take()
            .context("You already responded to this verification.")?
            .send(matches)
            .map_err(|_| anyhow::anyhow!("The private connection ended."))
    }

    pub fn send(&self, text: String) -> Result<()> {
        let state = self.state.lock().expect("private chat state");
        ensure!(
            matches!(state.status, Status::Connected),
            "Both people must verify the connection before sending private text."
        );
        ensure!(
            valid_text(&text),
            "Enter private text up to 2048 UTF-8 bytes."
        );
        self.outgoing
            .try_send(text)
            .map_err(|_| anyhow::anyhow!("Could not queue private text. Check the connection."))
    }

    pub fn finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn end(self) {
        let _ = self.cancel.send(());
        let _ = self.task.await;
    }
}

fn set_status(state: &Shared, status: Status) {
    state.lock().expect("private chat state").status = status;
}

fn failed(reason: impl Into<String>) -> Status {
    Status::Failed {
        reason: reason.into(),
    }
}

async fn run(
    host: bool,
    tag: [u8; 32],
    state: &Shared,
    decision: oneshot::Receiver<bool>,
    messages: mpsc::Receiver<String>,
) -> Status {
    let link = match timeout(
        DISCOVERY_TIME,
        lan::connect(host, &tag, || {
            set_status(state, Status::Connecting);
        }),
    )
    .await
    {
        Ok(Ok(link)) => link,
        Ok(Err(error)) => return failed(error.to_string()),
        Err(_) => {
            return failed(
                "Could not find the contact on this LAN. Both devices must start discovery.",
            );
        }
    };
    let conn = &link.connection;
    let result = tokio::select! {
        biased;
        result = exchange(conn, host, &tag, state, decision, messages) => result,
        _ = conn.closed() => Ok(()),
    };

    if let Some(ConnectionError::ApplicationClosed(close)) = conn.close_reason() {
        return match close.error_code.into_inner() {
            code if code == u64::from(END) => {
                if matches!(
                    state.lock().expect("private chat state").status,
                    Status::Connected
                ) {
                    Status::Ended
                } else {
                    failed("The contact ended the session before verification.")
                }
            }
            code if code == u64::from(DECLINED) => {
                failed("The contact declined identity verification.")
            }
            _ => failed("The contact lost the private connection."),
        };
    }
    conn.close(FAILED.into(), b"");
    failed(result.err().map_or_else(
        || "The private connection was lost.".into(),
        |error| error.to_string(),
    ))
}

async fn exchange(
    conn: &Connection,
    host: bool,
    tag: &[u8; 32],
    state: &Shared,
    decision: oneshot::Receiver<bool>,
    messages: mpsc::Receiver<String>,
) -> Result<()> {
    let (send, recv) = timeout(
        VERIFICATION_TIME,
        verify_connection(conn, host, tag, state, decision),
    )
    .await
    .context("Private verification timed out.")??;
    {
        let mut state = state.lock().expect("private chat state");
        state.last_text = Instant::now();
        state.status = Status::Connected;
    }
    tokio::select! {
        result = receive_text(recv, state) => result,
        result = send_text(send, state, messages) => result,
        result = expire_idle_session(state) => result,
    }
}

async fn expire_idle_session(state: &Shared) -> Result<()> {
    loop {
        let deadline = state.lock().expect("private chat state").last_text + SESSION_IDLE;
        ensure!(
            Instant::now() < deadline,
            "The private session timed out after 15 minutes without messages."
        );
        sleep_until(deadline).await;
    }
}

async fn verify_connection(
    conn: &Connection,
    host: bool,
    tag: &[u8; 32],
    state: &Shared,
    decision: oneshot::Receiver<bool>,
) -> Result<(SendStream, RecvStream)> {
    let (mut send, mut recv) = if host {
        conn.accept_bi().await
    } else {
        conn.open_bi().await
    }
    .context("Could not open the private verification stream.")?;
    send.write_all(tag)
        .await
        .context("Private verification was interrupted.")?;
    let mut peer_tag = [0; 32];
    recv.read_exact(&mut peer_tag)
        .await
        .context("Private verification was interrupted.")?;
    ensure!(
        &peer_tag == tag,
        "The device does not match this conversation."
    );

    let mut material = [0; 16];
    conn.export_keying_material(&mut material, b"EXPORTER-Zibblu-Private-v1", tag)
        .map_err(|_| anyhow::anyhow!("Could not generate a private verification code."))?;
    let code = material
        .chunks_exact(2)
        .map(|pair| format!("{:02X}{:02X}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join(" ");
    set_status(state, Status::Verify { code: code.clone() });

    let local = async {
        if !decision
            .await
            .context("Private verification was canceled.")?
        {
            conn.close(DECLINED.into(), b"");
            bail!("You declined identity verification.");
        }
        send.write_all(&[1])
            .await
            .context("Private verification was interrupted.")?;
        set_status(state, Status::WaitingForPeer { code });
        Ok(())
    };
    let remote = async {
        let mut answer = [0];
        recv.read_exact(&mut answer)
            .await
            .context("Private verification was interrupted.")?;
        ensure!(answer == [1], "The contact did not approve verification.");
        Ok(())
    };
    tokio::try_join!(local, remote)?;
    Ok((send, recv))
}

fn valid_text(text: &str) -> bool {
    text.len() <= MAX_TEXT_BYTES && !text.trim().is_empty()
}

async fn receive_text(mut recv: RecvStream, state: &Shared) -> Result<()> {
    loop {
        let mut size = [0; 2];
        recv.read_exact(&mut size)
            .await
            .context("The private connection was lost.")?;
        let size = usize::from(u16::from_be_bytes(size));
        ensure!(
            (1..=MAX_TEXT_BYTES).contains(&size),
            "The contact sent an invalid private message."
        );
        let mut bytes = vec![0; size];
        recv.read_exact(&mut bytes)
            .await
            .context("The private connection was lost.")?;
        let text = String::from_utf8(bytes).context("The contact sent invalid private text.")?;
        ensure!(valid_text(&text), "The contact sent invalid private text.");
        state
            .lock()
            .expect("private chat state")
            .record(false, text);
    }
}

async fn send_text(
    mut send: SendStream,
    state: &Shared,
    mut messages: mpsc::Receiver<String>,
) -> Result<()> {
    while let Some(text) = messages.recv().await {
        let size = (text.len() as u16).to_be_bytes();
        send.write_all(&size)
            .await
            .context("Could not send private text.")?;
        send.write_all(text.as_bytes())
            .await
            .context("Could not send private text.")?;
        state.lock().expect("private chat state").record(true, text);
    }
    Ok(())
}
