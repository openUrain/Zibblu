mod identity;
mod lan;

use crate::{LanCredential, MAX_GROUP_MEMBERS, User, valid_handle};
use anyhow::{Context, Result, bail, ensure};
pub use identity::DeviceKey;
use identity::{Consent, Identity};
use lan::{Lan, Scope};
use quinn::{Connection, ConnectionError, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::{Instant, interval, timeout},
};
use uuid::Uuid;

const ROOM_IDLE: Duration = Duration::from_secs(15 * 60);
const INVITATION_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_ROOM_COUNT: usize = 24;
const MAX_DEVICE_COUNT: usize = 48;
const MAX_TEXT: usize = 2048;
const MAX_FRAME: usize = 32 * 1024;
const HISTORY_LIMIT: usize = 100;
const APP_EXIT: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
pub struct PrivateCode {
    room: Uuid,
    host: Uuid,
    addresses: Vec<SocketAddr>,
    secret: Uuid,
    pub expires_at: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Discovering,
    Waiting,
    Invited,
    Authenticating,
    Connected,
    Missed,
    Failed,
    Ended,
}

#[derive(Clone, Serialize)]
pub struct PrivateSummary {
    pub id: Uuid,
    conversation: Option<Uuid>,
    host: User,
    invited: Vec<User>,
    participants: Vec<User>,
    pub phase: Phase,
    open: bool,
    hosted: bool,
}

#[derive(Clone, Serialize)]
struct PrivateMessage {
    seq: u64,
    sender: User,
    text: String,
}

#[derive(Serialize)]
pub struct PrivateView {
    #[serde(flatten)]
    summary: PrivateSummary,
    connection: Uuid,
    messages: Vec<PrivateMessage>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Room {
    id: Uuid,
    conversation: Option<Uuid>,
    host: Uuid,
    host_device: [u8; 32],
    invited: Vec<Uuid>,
    open: bool,
}

impl Room {
    fn proof_message(&self) -> Result<Vec<u8>> {
        let mut bytes = b"zibblu-private-approval-v2\0".to_vec();
        bytes.extend(serde_json::to_vec(self)?);
        Ok(bytes)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Wire {
    Offer {
        room: Room,
        decided: bool,
    },
    Scan {
        room: Uuid,
        secret: Uuid,
    },
    Decision {
        approval: Option<Consent>,
    },
    Joined {
        approval: Consent,
        participants: Vec<User>,
    },
    Participants {
        participants: Vec<User>,
    },
    Say {
        text: String,
    },
    Message {
        sender: User,
        text: String,
    },
    Leave,
    Closed {
        declined: bool,
    },
}

struct Feed {
    seq: u64,
    messages: VecDeque<PrivateMessage>,
    last_text: Instant,
}

impl Feed {
    fn new() -> Self {
        Self {
            seq: 0,
            messages: VecDeque::new(),
            last_text: Instant::now(),
        }
    }

    fn add(&mut self, sender: User, text: String) {
        self.seq += 1;
        self.last_text = Instant::now();
        if self.messages.len() == HISTORY_LIMIT {
            self.messages.pop_front();
        }
        self.messages.push_back(PrivateMessage {
            seq: self.seq,
            sender,
            text,
        });
    }

    fn since(&self, after: u64) -> Vec<PrivateMessage> {
        self.messages
            .iter()
            .filter(|message| message.seq > after)
            .cloned()
            .collect()
    }
}

struct Link {
    sender: mpsc::Sender<Wire>,
    user: User,
    active: bool,
    connection: usize,
}

struct Hosted {
    room: Room,
    invited: Vec<User>,
    decisions: HashMap<Uuid, Option<Consent>>,
    links: HashMap<[u8; 32], Link>,
    attempts: HashSet<SocketAddr>,
    code: Option<(Uuid, Instant, u64)>,
    feed: Feed,
    ever_connected: bool,
}

impl Hosted {
    fn has_peer(&self, own: Uuid) -> bool {
        self.links
            .values()
            .any(|link| link.active && link.user.id != own)
    }

    fn participants(&self, own: &User) -> Vec<User> {
        let mut participants = vec![own.clone()];
        for link in self.links.values().filter(|link| link.active) {
            if !participants.iter().any(|user| user.id == link.user.id) {
                participants.push(link.user.clone());
            }
        }
        participants.sort_by(|left, right| left.handle.cmp(&right.handle));
        participants
    }

    fn summary(&self, own: &User) -> PrivateSummary {
        PrivateSummary {
            id: self.room.id,
            conversation: self.room.conversation,
            host: own.clone(),
            invited: self.invited.clone(),
            participants: self.participants(own),
            phase: if self.has_peer(own.id) {
                Phase::Connected
            } else if !self.room.open && self.links.is_empty() {
                Phase::Discovering
            } else {
                Phase::Waiting
            },
            open: self.room.open,
            hosted: true,
        }
    }

    fn update_participants(&mut self, own: &User) {
        let participants = self.participants(own);
        self.links.retain(|_, link| {
            !link.active
                || link
                    .sender
                    .try_send(Wire::Participants {
                        participants: participants.clone(),
                    })
                    .is_ok()
        });
        if !self.has_peer(own.id) {
            self.feed.messages.clear();
        }
    }

    fn deliver(&mut self, sender: User, text: String) {
        self.feed.add(sender.clone(), text.clone());
        self.links.retain(|_, link| {
            !link.active
                || link
                    .sender
                    .try_send(Wire::Message {
                        sender: sender.clone(),
                        text: text.clone(),
                    })
                    .is_ok()
        });
    }

    fn close(self) {
        for link in self.links.into_values() {
            let _ = link.sender.try_send(Wire::Closed { declined: false });
        }
    }

    fn all_declined(&self) -> bool {
        !self.room.open
            && self
                .room
                .invited
                .iter()
                .all(|account| self.decisions.get(account).is_some_and(Option::is_none))
    }

    fn valid_qr(&self, secret: Uuid, now: Instant) -> bool {
        self.room.open
            && self
                .code
                .is_some_and(|(code, deadline, _)| code == secret && now < deadline)
    }
}

struct Guest {
    room: Room,
    summary: PrivateSummary,
    link: Uuid,
    sender: Option<mpsc::Sender<Wire>>,
    voted: bool,
    feed: Feed,
}

#[derive(Default)]
struct State {
    memberships: HashMap<Uuid, Vec<User>>,
    hosted: HashMap<Uuid, Hosted>,
    guests: HashMap<Uuid, Guest>,
}

struct Engine {
    own: User,
    lan: Lan,
    identity: Mutex<Arc<Identity>>,
    state: Mutex<State>,
}

impl Engine {
    fn identity(&self) -> Arc<Identity> {
        self.identity.lock().expect("LAN identity").clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("private sessions")
    }
}

pub struct PrivateLobby {
    engine: Arc<Engine>,
    task: JoinHandle<()>,
}

impl PrivateLobby {
    pub fn new(
        origin: String,
        own: User,
        key: DeviceKey,
        credential: LanCredential,
        memberships: HashMap<Uuid, Vec<User>>,
    ) -> Result<Self> {
        let identity = Identity::new(own.id, key, credential)?;
        let lan = Lan::new(Scope::current()?, &identity, origin, own.id)?;
        let engine = Arc::new(Engine {
            own,
            lan,
            identity: Mutex::new(Arc::new(identity)),
            state: Mutex::new(State {
                memberships,
                ..State::default()
            }),
        });
        let task = tokio::spawn(background(engine.clone()));
        Ok(Self { engine, task })
    }

    pub fn network_matches(&self) -> bool {
        Scope::current().is_ok_and(|scope| self.engine.lan.scope == scope)
    }

    pub fn renew(&self, key: DeviceKey, credential: LanCredential) -> Result<()> {
        let identity = Identity::new(self.engine.own.id, key, credential)?;
        self.engine.lan.set_identity(&identity);
        *self.engine.identity.lock().expect("LAN identity") = Arc::new(identity);
        Ok(())
    }

    pub fn set_memberships(&self, memberships: HashMap<Uuid, Vec<User>>) {
        self.engine.state().memberships = memberships;
    }

    pub fn list(&self) -> Vec<PrivateSummary> {
        let state = self.engine.state();
        let mut rooms: Vec<_> = state
            .hosted
            .values()
            .map(|room| room.summary(&self.engine.own))
            .chain(state.guests.values().map(|guest| guest.summary.clone()))
            .collect();
        rooms.sort_by_key(|room| room.id);
        rooms
    }

    pub fn view(&self, id: Uuid, after: u64) -> Result<PrivateView> {
        let state = self.engine.state();
        if let Some(room) = state.hosted.get(&id) {
            Ok(PrivateView {
                summary: room.summary(&self.engine.own),
                connection: id,
                messages: room.feed.since(after),
            })
        } else {
            let guest = state
                .guests
                .get(&id)
                .context("The private session has ended.")?;
            Ok(PrivateView {
                summary: guest.summary.clone(),
                connection: guest.link,
                messages: guest.feed.since(after),
            })
        }
    }

    pub fn create(
        &self,
        conversation: Option<Uuid>,
        mut invited: Vec<Uuid>,
        open: bool,
    ) -> Result<Uuid> {
        invited.sort_unstable();
        invited.dedup();
        let own = &self.engine.own;
        let identity = self.engine.identity();
        let mut state = self.engine.state();
        ensure!(
            state.hosted.len() < MAX_ROOM_COUNT,
            "End a private chat before starting another."
        );
        let room = Room {
            id: Uuid::new_v4(),
            conversation,
            host: own.id,
            host_device: identity.device,
            invited,
            open,
        };
        let people = validate_room(&state.memberships, own.id, &room)?;
        let approval = identity.sign(&room.proof_message()?)?;
        let id = room.id;
        state.hosted.insert(
            id,
            Hosted {
                room,
                invited: people,
                decisions: HashMap::from([(own.id, Some(approval))]),
                links: HashMap::new(),
                attempts: HashSet::new(),
                code: None,
                feed: Feed::new(),
                ever_connected: false,
            },
        );
        Ok(id)
    }

    pub fn code(&self, id: Uuid) -> Result<PrivateCode> {
        let mut state = self.engine.state();
        let hosted = state
            .hosted
            .get_mut(&id)
            .context("Open a private QR session first.")?;
        ensure!(
            hosted.room.open,
            "This private chat uses named invitations."
        );
        let code = hosted.code.get_or_insert_with(new_code);
        if Instant::now() >= code.1 {
            *code = new_code();
        }
        let (secret, _, expires_at) = *code;
        Ok(PrivateCode {
            room: id,
            host: self.engine.own.id,
            addresses: self.engine.lan.addresses().to_vec(),
            secret,
            expires_at,
        })
    }

    pub async fn scan(&self, code: PrivateCode) -> Result<Uuid> {
        ensure!(
            code.expires_at > now_millis(),
            "This private QR code has expired."
        );
        ensure!(
            (1..=16).contains(&code.addresses.len()),
            "Invalid private QR code."
        );
        for address in code.addresses {
            let identity = self.engine.identity();
            let Ok(connection) = self.engine.lan.connect(&identity, code.host, address).await
            else {
                continue;
            };
            let result = timeout(Duration::from_secs(4), async {
                let (mut send, mut recv) = connection.open_bi().await?;
                send_frame(
                    &mut send,
                    Wire::Scan {
                        room: code.room,
                        secret: code.secret,
                    },
                )
                .await?;
                let Wire::Offer { room, decided } = recv_frame(&mut recv).await? else {
                    bail!("This QR session is unavailable.");
                };
                ensure!(
                    room.id == code.room && room.open,
                    "This QR code does not match the private session."
                );
                attach_guest_task(&self.engine, connection, send, recv, room, true, decided)?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if let Ok(Ok(())) = result {
                return Ok(code.room);
            }
        }
        bail!("The private QR session is unavailable on this LAN.")
    }

    pub fn respond(&self, id: Uuid, accept: bool) -> Result<Option<Uuid>> {
        let identity = self.engine.identity();
        let mut state = self.engine.state();
        let guest = state
            .guests
            .get_mut(&id)
            .context("This invitation is unavailable.")?;
        ensure!(
            guest.summary.phase == Phase::Invited && !guest.voted,
            "This invitation already has a response."
        );
        let approval = accept
            .then(|| identity.sign(&guest.room.proof_message()?))
            .transpose()?;
        guest
            .sender
            .as_ref()
            .context("The invitation ended.")?
            .try_send(Wire::Decision { approval })?;
        guest.voted = true;
        guest.summary.phase = if accept {
            Phase::Authenticating
        } else {
            Phase::Waiting
        };
        Ok(guest.room.conversation)
    }

    pub fn send(&self, id: Uuid, text: String) -> Result<()> {
        ensure!(valid_text(&text), "Enter private text up to 2 KiB.");
        let mut state = self.engine.state();
        if let Some(room) = state.hosted.get_mut(&id) {
            ensure!(
                room.summary(&self.engine.own).phase == Phase::Connected,
                "A participant must join first."
            );
            room.deliver(self.engine.own.clone(), text);
            Ok(())
        } else {
            let guest = state
                .guests
                .get(&id)
                .context("The private session has ended.")?;
            ensure!(
                guest.summary.phase == Phase::Connected,
                "Join the private session first."
            );
            Ok(guest
                .sender
                .as_ref()
                .context("The connection closed.")?
                .try_send(Wire::Say { text })?)
        }
    }

    pub fn end(&self, id: Uuid) {
        let mut state = self.engine.state();
        if let Some(room) = state.hosted.remove(&id) {
            room.close();
        }
        if let Some(mut guest) = state.guests.remove(&id) {
            if let Some(sender) = guest.sender.take() {
                let _ = sender.try_send(Wire::Leave);
            }
        }
    }

    pub fn shutdown(&self) {
        self.stop(0);
    }

    pub fn quit(&self) {
        self.stop(APP_EXIT);
    }

    fn stop(&self, code: u32) {
        self.task.abort();
        self.engine.lan.endpoint.close(code.into(), b"");
        let mut state = self.engine.state();
        state.hosted.clear();
        state.guests.clear();
    }
}

impl Drop for PrivateLobby {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn new_code() -> (Uuid, Instant, u64) {
    (
        Uuid::new_v4(),
        Instant::now() + ROOM_IDLE,
        now_millis() + ROOM_IDLE.as_millis() as u64,
    )
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn validate_room(
    memberships: &HashMap<Uuid, Vec<User>>,
    own: Uuid,
    room: &Room,
) -> Result<Vec<User>> {
    if room.open {
        ensure!(
            room.invited.is_empty() && room.conversation.is_none(),
            "Invalid private QR room."
        );
        return Ok(Vec::new());
    }
    ensure!(
        (1..MAX_GROUP_MEMBERS).contains(&room.invited.len()),
        "Select at least one participant."
    );
    ensure!(
        room.invited.windows(2).all(|pair| pair[0] < pair[1]) && !room.invited.contains(&room.host),
        "Invalid private participants."
    );
    ensure!(
        own == room.host || room.invited.contains(&own),
        "This invitation belongs to another account."
    );
    let members = room
        .conversation
        .and_then(|id| memberships.get(&id))
        .context("Open an existing conversation first.")?;
    ensure!(
        members.iter().any(|user| user.id == room.host),
        "The sender is outside this conversation."
    );
    room.invited
        .iter()
        .map(|id| {
            members
                .iter()
                .find(|user| user.id == *id)
                .cloned()
                .context("A participant is outside this conversation.")
        })
        .collect()
}

fn valid_roster(room: &Room, participants: &[User], own: Uuid) -> bool {
    let ids: HashSet<_> = participants.iter().map(|user| user.id).collect();
    ids.len() == participants.len()
        && ids.len() <= MAX_DEVICE_COUNT + 1
        && ids.contains(&room.host)
        && ids.contains(&own)
        && participants.iter().all(|user| {
            valid_handle(&user.handle)
                && (room.open || user.id == room.host || room.invited.contains(&user.id))
        })
}

async fn background(engine: Arc<Engine>) {
    let mut tick = interval(Duration::from_secs(2));
    let connections = Arc::new(Semaphore::new(128));
    let mut discovery = None::<JoinHandle<()>>;
    loop {
        tokio::select! {
            incoming = engine.lan.endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                if let Ok(permit) = connections.clone().try_acquire_owned() {
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        if let Ok(Ok(connection)) = timeout(Duration::from_secs(4), incoming).await {
                            let _ = inbound(engine, connection, permit).await;
                        }
                    });
                }
            }
            _ = tick.tick() => {
                if discovery.as_ref().is_none_or(|task| task.is_finished()) {
                    discovery = Some(tokio::spawn(discover(engine.clone())));
                }
            }
        }
    }
}

async fn discover(engine: Arc<Engine>) {
    let accounts: Vec<_> = {
        let mut state = engine.state();
        state.hosted.retain(|_, host| {
            let limit = if host.ever_connected || host.room.open {
                ROOM_IDLE
            } else {
                INVITATION_TIMEOUT
            };
            let live = host.feed.last_text.elapsed() < limit;
            if !live {
                for link in host.links.values() {
                    let _ = link.sender.try_send(Wire::Closed { declined: false });
                }
            }
            live
        });
        state
            .hosted
            .values()
            .flat_map(|host| {
                host.room
                    .invited
                    .iter()
                    .copied()
                    .chain(host.decisions.keys().copied())
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    };
    if accounts.is_empty() {
        return;
    }
    let peers = engine.lan.candidates(&accounts).await;
    let mut attempts = Vec::new();
    {
        let mut state = engine.state();
        for host in state.hosted.values_mut() {
            for &(account, address) in &peers {
                if host.attempts.len() >= MAX_DEVICE_COUNT {
                    break;
                }
                if host.decisions.get(&account).is_some_and(Option::is_none) {
                    continue;
                }
                if (host.room.invited.contains(&account) || host.decisions.contains_key(&account))
                    && host.attempts.insert(address)
                {
                    attempts.push((host.room.id, account, address));
                }
            }
        }
    }
    for (room, account, address) in attempts {
        let engine = engine.clone();
        tokio::spawn(async move {
            let _ = outbound(&engine, room, account, address).await;
            if let Some(host) = engine.state().hosted.get_mut(&room) {
                host.attempts.remove(&address);
            }
        });
    }
}

async fn outbound(
    engine: &Arc<Engine>,
    room: Uuid,
    account: Uuid,
    address: SocketAddr,
) -> Result<()> {
    let connection = engine
        .lan
        .connect(&engine.identity(), account, address)
        .await?;
    let (send, recv) = connection.open_bi().await?;
    host_link(engine, connection, send, recv, room, None).await
}

async fn inbound(
    engine: Arc<Engine>,
    connection: Connection,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let (send, recv, frame) = timeout(Duration::from_secs(4), async {
        let (send, mut recv) = connection.accept_bi().await?;
        let frame = recv_frame(&mut recv).await?;
        Ok::<_, anyhow::Error>((send, recv, frame))
    })
    .await??;
    match frame {
        Wire::Offer { room, decided } => {
            let task = attach_guest_task(&engine, connection, send, recv, room, false, decided)?;
            let _ = task.await;
            Ok(())
        }
        Wire::Scan { room, secret } => {
            host_link(&engine, connection, send, recv, room, Some(secret)).await
        }
        _ => bail!("Invalid private connection."),
    }
}

async fn host_link(
    engine: &Arc<Engine>,
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    id: Uuid,
    qr_secret: Option<Uuid>,
) -> Result<()> {
    let peer = identity::peer(&connection)?;
    let account = peer.user.id;
    let device = peer.device;
    let connection_id = connection.stable_id();
    let (sender, outgoing) = mpsc::channel(32);
    {
        let mut state = engine.state();
        let host = state
            .hosted
            .get_mut(&id)
            .context("The private session ended.")?;
        ensure!(
            device != host.room.host_device
                && !host.links.contains_key(&device)
                && host.links.len() < MAX_DEVICE_COUNT,
            "Device already connected."
        );
        if let Some(secret) = qr_secret {
            ensure!(
                host.valid_qr(secret, Instant::now()),
                "The private QR code expired."
            );
        } else {
            ensure!(
                host.room.invited.contains(&account) || host.decisions.contains_key(&account),
                "This account is outside the invitation."
            );
        }
        ensure!(
            !host.decisions.get(&account).is_some_and(Option::is_none),
            "This invitation was declined."
        );
        let approval = host.decisions.get(&account).and_then(|value| value.clone());
        sender.try_send(Wire::Offer {
            room: host.room.clone(),
            decided: approval.is_some(),
        })?;
        host.links.insert(
            device,
            Link {
                sender,
                user: peer.user,
                active: approval.is_some(),
                connection: connection_id,
            },
        );
        if let Some(approval) = approval {
            admit(host, &engine.own, account, approval);
        }
    }

    let result = pump(&connection, send, recv, outgoing, |frame| {
        let identity = engine.identity();
        let mut state = engine.state();
        let host = state
            .hosted
            .get_mut(&id)
            .context("The private session ended.")?;
        let link = host
            .links
            .get(&device)
            .filter(|link| link.connection == connection_id)
            .context("The device left.")?;
        match frame {
            Wire::Decision { approval } if !host.decisions.contains_key(&account) => {
                if let Some(approval) = approval {
                    if let Some(secret) = qr_secret {
                        ensure!(
                            host.valid_qr(secret, Instant::now()),
                            "The private QR code expired."
                        );
                    }
                    identity.verify(&approval, &host.room.proof_message()?, account, true)?;
                    let count = host
                        .decisions
                        .values()
                        .filter(|entry| entry.is_some())
                        .count();
                    ensure!(count < MAX_DEVICE_COUNT, "This private group is full.");
                    host.decisions.insert(account, Some(approval.clone()));
                    for link in host
                        .links
                        .values_mut()
                        .filter(|link| link.user.id == account)
                    {
                        link.active = true;
                    }
                    admit(host, &engine.own, account, approval);
                } else {
                    reject_account(host, &engine.own, account);
                }
            }
            Wire::Decision { .. } => {}
            Wire::Say { text } if link.active && valid_text(&text) => {
                host.deliver(link.user.clone(), text);
            }
            Wire::Leave => reject_account(host, &engine.own, account),
            _ => bail!("Invalid private message."),
        }
        Ok(())
    })
    .await;
    let quit = matches!(connection.close_reason(), Some(ConnectionError::ApplicationClosed(close))
        if close.error_code.into_inner() == u64::from(APP_EXIT));
    let mut state = engine.state();
    if let Some(host) = state.hosted.get_mut(&id) {
        let mut active = false;
        if host
            .links
            .get(&device)
            .is_some_and(|link| link.connection == connection_id)
        {
            active = host.links.remove(&device).is_some_and(|link| link.active);
        }
        if quit
            && active
            && account != engine.own.id
            && !host
                .links
                .values()
                .any(|link| link.user.id == account && link.active)
        {
            reject_account(host, &engine.own, account);
        } else {
            host.update_participants(&engine.own);
        }
        if host.all_declined()
            && let Some(host) = state.hosted.remove(&id)
        {
            host.close();
        }
    }
    result
}

fn admit(host: &mut Hosted, own: &User, account: Uuid, approval: Consent) {
    if account != own.id && !host.ever_connected {
        host.ever_connected = true;
        host.feed.last_text = Instant::now();
    }
    let participants = host.participants(own);
    host.links.retain(|_, link| {
        link.user.id != account
            || link
                .sender
                .try_send(Wire::Joined {
                    approval: approval.clone(),
                    participants: participants.clone(),
                })
                .is_ok()
    });
    host.update_participants(own);
}

fn reject_account(host: &mut Hosted, own: &User, account: Uuid) {
    host.decisions.insert(account, None);
    host.links.retain(|_, link| {
        if link.user.id != account {
            return true;
        }
        let _ = link.sender.try_send(Wire::Closed { declined: true });
        false
    });
    host.update_participants(own);
}

fn attach_guest_task(
    engine: &Arc<Engine>,
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    room: Room,
    scanned: bool,
    decided: bool,
) -> Result<JoinHandle<()>> {
    let peer = identity::peer(&connection)?;
    ensure!(
        peer.user.id == room.host && peer.device == room.host_device,
        "Unexpected invitation sender."
    );
    let id = room.id;
    let link = Uuid::new_v4();
    let (sender, outgoing) = mpsc::channel(32);
    {
        let mut state = engine.state();
        let named = validate_room(&state.memberships, engine.own.id, &room)?;
        if let Some(old) = state.guests.get(&id) {
            ensure!(
                old.room == room && old.sender.is_none(),
                "A private connection already exists."
            );
        }
        if state.guests.len() >= MAX_ROOM_COUNT {
            state.guests.retain(|_, guest| guest.sender.is_some());
        }
        ensure!(
            state.guests.len() < MAX_ROOM_COUNT,
            "Too many private invitations."
        );
        if scanned {
            let approval = engine.identity().sign(&room.proof_message()?)?;
            sender.try_send(Wire::Decision {
                approval: Some(approval),
            })?;
        }
        let phase = if decided || room.open || room.host == engine.own.id || scanned {
            Phase::Authenticating
        } else {
            Phase::Invited
        };
        let summary = PrivateSummary {
            id,
            conversation: room.conversation,
            host: peer.user,
            invited: named,
            participants: Vec::new(),
            phase,
            open: room.open,
            hosted: false,
        };
        state.guests.insert(
            id,
            Guest {
                room,
                summary,
                link,
                sender: Some(sender),
                voted: scanned,
                feed: Feed::new(),
            },
        );
    }
    let engine = engine.clone();
    Ok(tokio::spawn(async move {
        let _ = pump(&connection, send, recv, outgoing, |frame| {
            let identity = engine.identity();
            let mut state = engine.state();
            let guest = state
                .guests
                .get_mut(&id)
                .filter(|guest| guest.link == link)
                .context("The private session ended.")?;
            match frame {
                Wire::Joined {
                    approval,
                    participants,
                } => {
                    identity.verify(
                        &approval,
                        &guest.room.proof_message()?,
                        engine.own.id,
                        false,
                    )?;
                    ensure!(
                        valid_roster(&guest.room, &participants, engine.own.id),
                        "Invalid private roster."
                    );
                    guest.summary.phase = Phase::Connected;
                    guest.summary.participants = participants;
                }
                Wire::Participants { participants } if guest.summary.phase == Phase::Connected => {
                    ensure!(
                        valid_roster(&guest.room, &participants, engine.own.id),
                        "Invalid private roster."
                    );
                    guest.summary.participants = participants;
                }
                Wire::Message { sender, text }
                    if guest.summary.phase == Phase::Connected
                        && valid_text(&text)
                        && guest.summary.participants.contains(&sender) =>
                {
                    guest.feed.add(sender, text);
                }
                Wire::Closed { declined } => {
                    guest.summary.phase = if !declined && guest.summary.phase == Phase::Invited {
                        Phase::Missed
                    } else {
                        Phase::Ended
                    };
                    bail!("Private session closed.");
                }
                _ => bail!("Invalid private message."),
            }
            Ok(())
        })
        .await;
        if let Some(guest) = engine
            .state()
            .guests
            .get_mut(&id)
            .filter(|guest| guest.link == link)
        {
            let quit = matches!(connection.close_reason(), Some(ConnectionError::ApplicationClosed(close))
                if close.error_code.into_inner() == u64::from(APP_EXIT));
            guest.summary.phase = match guest.summary.phase {
                Phase::Invited | Phase::Missed => Phase::Missed,
                Phase::Ended => Phase::Ended,
                _ if quit => Phase::Ended,
                _ => Phase::Failed,
            };
            guest.summary.participants.clear();
            guest.feed = Feed::new();
            guest.sender.take();
        }
    }))
}

async fn pump(
    connection: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    mut outgoing: mpsc::Receiver<Wire>,
    mut receive: impl FnMut(Wire) -> Result<()>,
) -> Result<()> {
    let reader = async {
        loop {
            receive(recv_frame(&mut recv).await?)?;
        }
    };
    let writer = async {
        while let Some(frame) = outgoing.recv().await {
            send_frame(&mut send, frame).await?;
        }
        send.finish()?;
        let _ = timeout(Duration::from_secs(1), send.stopped()).await;
        Ok(())
    };
    tokio::select! {
        result = reader => result,
        result = writer => result,
        _ = connection.closed() => Ok(()),
    }
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_TEXT && !text.contains('\0')
}

async fn send_frame(send: &mut SendStream, frame: Wire) -> Result<()> {
    let bytes = serde_json::to_vec(&frame)?;
    ensure!(bytes.len() <= MAX_FRAME, "Private frame too large.");
    send.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    Ok(())
}

async fn recv_frame(recv: &mut RecvStream) -> Result<Wire> {
    let mut size = [0; 4];
    recv.read_exact(&mut size).await?;
    let size = u32::from_be_bytes(size) as usize;
    ensure!(
        (1..=MAX_FRAME).contains(&size),
        "Invalid private frame size."
    );
    let mut bytes = vec![0; size];
    recv.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes).context("Invalid private frame.")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_qr_expires_at_its_monotonic_deadline() {
        let code = new_code();
        let (secret, deadline, _) = code;
        let host = Hosted {
            room: Room {
                id: Uuid::new_v4(),
                conversation: None,
                host: Uuid::new_v4(),
                host_device: [0; 32],
                invited: Vec::new(),
                open: true,
            },
            invited: Vec::new(),
            decisions: HashMap::new(),
            links: HashMap::new(),
            attempts: HashSet::new(),
            code: Some(code),
            feed: Feed::new(),
            ever_connected: false,
        };
        assert!(host.valid_qr(secret, deadline - Duration::from_millis(1)));
        assert!(!host.valid_qr(secret, deadline));
    }
}
