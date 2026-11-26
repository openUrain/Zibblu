mod identity;
mod lan;

use crate::{LAN_NEARBY_NAME, LanCredential, MAX_GROUP_MEMBERS, User, lan_peer_name, valid_handle};
use anyhow::{Context, Result, bail, ensure};
pub use identity::DeviceKey;
use identity::{Consent, Identity};
use lan::{Lan, Scope, discovery_instance};
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
    task::{JoinHandle, JoinSet},
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
    Suspended,
    Reconnecting,
    Missed,
    Failed,
    Ended,
}

#[derive(Clone, Serialize)]
pub struct PrivateSummary {
    pub id: Uuid,
    conversation: Option<Uuid>,
    host: User,
    pub invited: Vec<User>,
    participants: Vec<User>,
    pub phase: Phase,
    unread: bool,
    open: bool,
    hosted: bool,
}

#[derive(Clone, Serialize)]
struct PrivateMessage {
    seq: u64,
    sender: User,
    text: String,
    created_at: u64,
}

#[derive(Serialize)]
pub struct PrivateView {
    #[serde(flatten)]
    summary: PrivateSummary,
    connection: Uuid,
    messages: Vec<PrivateMessage>,
}

#[derive(Serialize)]
pub struct NearbyDevice {
    pub id: Uuid,
    pub matched_account: Option<User>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    Starting,
    NetworkUnavailable,
    #[cfg(target_os = "ios")]
    PermissionDenied,
    DiscoveryFailed,
    Ready,
}

#[derive(Serialize)]
pub struct NearbyDevices {
    pub status: DiscoveryStatus,
    pub devices: Vec<NearbyDevice>,
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
    fn resumable(&self) -> bool {
        cfg!(windows) && !self.open && self.invited.len() == 1
    }

    fn direct_peer(&self, own: Uuid) -> Option<Uuid> {
        self.resumable().then(|| if self.host == own { self.invited[0] } else { self.host })
    }

    fn standalone(&self) -> bool {
        !self.open && self.conversation.is_none()
    }

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
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        can_resume: bool,
    },
    Scan {
        room: Uuid,
        secret: Uuid,
    },
    Decision {
        approval: Option<Consent>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        can_resume: bool,
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
    seen: u64,
    messages: VecDeque<PrivateMessage>,
    last_text: Instant,
}

impl Feed {
    fn new() -> Self {
        Self {
            seq: 0,
            seen: 0,
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
            created_at: now_millis(),
        });
    }

    fn since(&self, after: u64) -> Vec<PrivateMessage> {
        self.messages
            .iter()
            .filter(|message| message.seq > after)
            .cloned()
            .collect()
    }

    fn unread(&self, own: Uuid) -> bool {
        self.messages.iter().rev().take_while(|message| message.seq > self.seen)
            .any(|message| message.sender.id != own)
    }

    fn mark_read(&mut self, through: u64) {
        self.seen = self.seen.max(through.min(self.seq));
    }
}

struct Link {
    sender: mpsc::Sender<Wire>,
    user: User,
    active: bool,
    connection: usize,
    transport: Connection,
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
    suspended_at: Option<Instant>,
    can_resume: bool,
    nearby: bool,
}

impl Hosted {
    fn has_peer(&self, own: Uuid) -> bool {
        self.links
            .values()
            .any(|link| link.active && link.user.id != own
                && (!self.can_resume || link.transport.close_reason().is_none()))
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
            } else if self.can_resume && self.ever_connected {
                if self.attempts.is_empty() { Phase::Suspended } else { Phase::Reconnecting }
            } else if !self.room.open && !self.nearby && self.links.is_empty() {
                Phase::Discovering
            } else {
                Phase::Waiting
            },
            unread: self.feed.unread(own.id),
            open: self.room.open,
            hosted: true,
        }
    }

    fn update_participants(&mut self, own: &User) {
        let participants = self.wire_participants(own);
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
            if self.can_resume && self.ever_connected {
                self.suspended_at.get_or_insert_with(Instant::now);
            } else {
                self.feed.messages.clear();
            }
        }
    }

    fn deliver(&mut self, sender: User, text: String) {
        self.feed.add(sender.clone(), text.clone());
        let mut sender = sender;
        sender.handle.make_ascii_lowercase();
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

    fn wire_participants(&self, own: &User) -> Vec<User> {
        let mut participants = self.participants(own);
        // Peers on the 0.1.0 wire format require lowercase handles.
        for user in &mut participants {
            user.handle.make_ascii_lowercase();
        }
        participants
    }

    fn close(self) {
        for link in self.links.into_values() {
            if link.sender.try_send(Wire::Closed { declined: false }).is_err() {
                link.transport.close(APP_EXIT.into(), b"");
            }
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
    transport: Option<Connection>,
    voted: bool,
    feed: Feed,
    suspended_at: Option<Instant>,
    can_resume: bool,
}

pub struct PrivateResponse {
    pub conversation: Option<Uuid>,
    pub applied: bool,
}

#[derive(Clone, Default)]
pub struct DismissedRooms(Arc<Mutex<HashSet<Uuid>>>);

impl DismissedRooms {
    pub fn record(&self, room: Uuid) {
        self.0.lock().expect("dismissed rooms").insert(room);
    }

    pub fn clear(&self) {
        self.0.lock().expect("dismissed rooms").clear();
    }

    fn admit(&self, room: Uuid, scanned: bool) -> Result<()> {
        let mut dismissed = self.0.lock().expect("dismissed rooms");
        if scanned {
            dismissed.remove(&room);
        } else {
            ensure!(
                !dismissed.contains(&room),
                "This private session was closed on this device."
            );
        }
        Ok(())
    }
}

#[derive(Default)]
struct State {
    memberships: HashMap<Uuid, Vec<User>>,
    hosted: HashMap<Uuid, Hosted>,
    guests: HashMap<Uuid, Guest>,
    dismissed: DismissedRooms,
}

impl State {
    fn end_guest(&mut self, id: Uuid) {
        self.dismissed.record(id);
        if let Some(guest) = self.guests.remove(&id)
            && !guest.sender.is_some_and(|sender| sender.try_send(Wire::Leave).is_ok())
            && let Some(transport) = guest.transport
        {
            transport.close(APP_EXIT.into(), b"");
        }
    }
}

struct Engine {
    own: User,
    lan: Mutex<Option<Arc<Lan>>>,
    identity: Mutex<Arc<Identity>>,
    state: Mutex<State>,
}

impl Engine {
    fn lan(&self) -> Result<Arc<Lan>> {
        self.lan.lock().expect("LAN transport").clone().context("The LAN is unavailable.")
    }

    fn identity(&self) -> Arc<Identity> {
        self.identity.lock().expect("LAN identity").clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("private sessions")
    }
}

pub struct PrivateLobby {
    engine: Arc<Engine>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl PrivateLobby {
    pub fn new(
        origin: String,
        own: User,
        key: DeviceKey,
        credential: LanCredential,
        memberships: HashMap<Uuid, Vec<User>>,
        dismissed: DismissedRooms,
    ) -> Result<Self> {
        let identity = Identity::new(own.id, key, credential)?;
        let lan = Arc::new(Lan::new(Scope::current()?, &identity, origin, own.id)?);
        let engine = Arc::new(Engine {
            own,
            lan: Mutex::new(Some(lan.clone())),
            identity: Mutex::new(Arc::new(identity)),
            state: Mutex::new(State {
                memberships,
                dismissed,
                ..State::default()
            }),
        });
        let task = Mutex::new(Some(tokio::spawn(background(engine.clone(), lan))));
        Ok(Self { engine, task })
    }

    pub fn network_matches(&self) -> bool {
        Scope::current().is_ok_and(|scope| self.engine.lan().is_ok_and(|lan| lan.scope == scope))
    }

    #[cfg(target_os = "ios")]
    pub fn lan_ready(&self) -> bool {
        self.network_matches() && self.engine.lan().is_ok_and(|lan| matches!(lan.discovery_status(), DiscoveryStatus::Ready))
    }

    pub fn renew(&self, key: DeviceKey, credential: LanCredential) -> Result<()> {
        let identity = Identity::new(self.engine.own.id, key, credential)?;
        if let Ok(lan) = self.engine.lan() {
            lan.set_identity(&identity);
        }
        *self.engine.identity.lock().expect("LAN identity") = Arc::new(identity);
        Ok(())
    }

    pub fn set_memberships(&self, memberships: HashMap<Uuid, Vec<User>>) {
        let mut state = self.engine.state();
        let revoked = |room: &Room| {
            room.conversation.is_some()
                && validate_room(&memberships, &self.engine.own, room, None).is_err()
        };
        let hosted: Vec<_> = state.hosted.values().filter(|host| revoked(&host.room)).map(|host| host.room.id).collect();
        let guests: Vec<_> = state.guests.values().filter(|guest| revoked(&guest.room)).map(|guest| guest.room.id).collect();
        for id in hosted {
            if let Some(host) = state.hosted.remove(&id) {
                host.close();
            }
        }
        for id in guests {
            state.end_guest(id);
        }
        state.memberships = memberships;
    }

    pub fn discovery_status() -> DiscoveryStatus {
        match Scope::current() {
            Ok(_) => DiscoveryStatus::Ready,
            Err(error) if error.is::<lan::NoLanInterface>() => DiscoveryStatus::NetworkUnavailable,
            Err(_) => DiscoveryStatus::DiscoveryFailed,
        }
    }

    pub async fn nearby_devices(&self) -> NearbyDevices {
        let unavailable = |status| NearbyDevices {
            status,
            devices: Vec::new(),
        };
        let Ok(lan) = self.engine.lan() else { return unavailable(DiscoveryStatus::NetworkUnavailable); };
        let status = lan.discovery_status();
        if !matches!(status, DiscoveryStatus::Ready) {
            return unavailable(status);
        }
        let records = lan.nearby(None).await;
        match Scope::current() {
            Ok(scope) if scope == lan.scope => {}
            Ok(_) => return unavailable(DiscoveryStatus::Starting),
            Err(error) if error.is::<lan::NoLanInterface>() => {
                return unavailable(DiscoveryStatus::NetworkUnavailable);
            }
            Err(_) => return unavailable(DiscoveryStatus::DiscoveryFailed),
        }
        let Ok(records) = records else {
            return unavailable(DiscoveryStatus::DiscoveryFailed);
        };
        let own = &self.engine.own;
        let state = self.engine.state();
        let accounts: HashMap<_, _> = state
            .memberships
            .values()
            .filter(|members| members.iter().any(|member| member.id == own.id))
            .flatten()
            .chain(std::iter::once(own))
            .map(|user| (lan.tag(user.id), user))
            .collect();
        let mut devices: Vec<_> = records
            .into_iter()
            .map(|(id, record)| NearbyDevice {
                id,
                matched_account: accounts.get(&record.tag).map(|&user| user.clone()),
            })
            .collect();
        devices.sort_by_key(|device| device.id);
        NearbyDevices {
            status: DiscoveryStatus::Ready,
            devices,
        }
    }

    pub async fn invite_nearby(&self, device: Uuid, account: Option<Uuid>) -> Result<Uuid> {
        let own = self.engine.own.id;
        ensure!(
            account != Some(own),
            "Select a device from another account."
        );
        let lan = self.engine.lan()?;
        ensure!(
            matches!(lan.discovery_status(), DiscoveryStatus::Ready),
            "Nearby discovery is unavailable."
        );
        let record = lan
            .nearby(Some(device))
            .await?
            .remove(&device)
            .filter(|record| account.is_none_or(|account| record.tag == lan.tag(account)))
            .context("This device is no longer listed.")?;
        ensure!(
            self.network_matches(),
            "The LAN changed. Wait for the list to refresh."
        );
        let mut attempts = JoinSet::new();
        let name = account
            .map(lan_peer_name)
            .unwrap_or_else(|| LAN_NEARBY_NAME.into());
        for address in record.addresses.into_iter().take(MAX_DEVICE_COUNT) {
            let engine = self.engine.clone();
            let lan = lan.clone();
            let name = name.clone();
            let tag = record.tag.clone();
            attempts.spawn(async move {
                let connection = lan
                    .connect_name(&engine.identity(), &name, address)
                    .await?;
                let certified = identity::peer(&connection)?;
                let peer = certified.user;
                ensure!(
                    peer.id != own
                        && account.is_none_or(|id| peer.id == id)
                        && (account.is_some() || discovery_instance(&certified.device) == device)
                        && lan.tag(peer.id) == tag,
                    "Unexpected device account."
                );
                Ok::<_, anyhow::Error>((connection, peer))
            });
        }
        let (connection, peer) = loop {
            match attempts.join_next().await {
                Some(Ok(Ok(connection))) => break connection,
                None => bail!(
                    "Could not authenticate the selected device over the LAN. Check that it is running and direct UDP is allowed."
                ),
                _ => {}
            }
        };
        drop(attempts);
        let (send, recv) = timeout(Duration::from_secs(4), connection.open_bi())
            .await
            .context("The selected device did not open a private connection.")??;
        let conversation = self
            .engine
            .state()
            .memberships
            .iter()
            .filter(|(_, members)| {
                (!cfg!(windows) || members.len() == 2) && members.iter().any(|user| user.id == own)
                    && members.iter().any(|user| user.id == peer.id)
            })
            .min_by_key(|(id, members)| (members.len(), **id))
            .map(|(&id, _)| id);
        let id = self.create_room(conversation, vec![peer.id], false, Some(peer))?;
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let _ = host_link(&engine, connection, send, recv, id, None).await;
            let mut state = engine.state();
            if state.hosted.get(&id).is_some_and(|host| !host.can_resume || !host.ever_connected)
                && let Some(host) = state.hosted.remove(&id) {
                host.close();
            }
        });
        Ok(id)
    }

    pub fn list(&self) -> Vec<PrivateSummary> {
        let state = self.engine.state();
        let mut rooms: Vec<_> = state
            .hosted
            .values()
            .map(|room| room.summary(&self.engine.own))
            .chain(state.guests.values().map(|guest| guest.summary(&self.engine.own)))
            .collect();
        for room in &mut rooms {
            restore_room_names(room, &state.memberships, &self.engine.own);
        }
        rooms.sort_by_key(|room| room.id);
        rooms
    }

    #[cfg(windows)]
    pub fn has_active_conversation(&self, conversation: Uuid) -> bool {
        self.list().iter().any(|room| room.conversation == Some(conversation)
            && matches!(room.phase, Phase::Connected | Phase::Suspended | Phase::Reconnecting))
    }

    pub fn view(&self, id: Uuid, after: u64) -> Result<PrivateView> {
        let state = self.engine.state();
        let mut view = if let Some(room) = state.hosted.get(&id) {
            PrivateView {
                summary: room.summary(&self.engine.own),
                connection: id,
                messages: room.feed.since(after),
            }
        } else {
            let guest = state
                .guests
                .get(&id)
                .context("The private session has ended.")?;
            PrivateView {
                summary: guest.summary(&self.engine.own),
                connection: guest.link,
                messages: guest.feed.since(after),
            }
        };
        restore_room_names(&mut view.summary, &state.memberships, &self.engine.own);
        let members = view
            .summary
            .conversation
            .and_then(|id| state.memberships.get(&id))
            .map(Vec::as_slice);
        for message in &mut view.messages {
            restore_name(&mut message.sender, members, &self.engine.own);
        }
        Ok(view)
    }

    pub fn mark_read(&self, id: Uuid, connection: Uuid, through: u64) -> bool {
        let mut state = self.engine.state();
        if let Some(host) = state.hosted.get_mut(&id) {
            if connection == id && host.has_peer(self.engine.own.id) {
                host.feed.mark_read(through);
                return true;
            }
        } else if let Some(guest) = state.guests.get_mut(&id)
            && guest.link == connection && guest.summary.phase == Phase::Connected
        {
            guest.feed.mark_read(through);
            return true;
        }
        false
    }

    pub fn create(
        &self,
        conversation: Option<Uuid>,
        invited: Vec<Uuid>,
        open: bool,
    ) -> Result<Uuid> {
        ensure!(
            open || conversation.is_some(),
            "Open an existing conversation first."
        );
        self.create_room(conversation, invited, open, None)
    }

    fn create_room(
        &self,
        conversation: Option<Uuid>,
        mut invited: Vec<Uuid>,
        open: bool,
        nearby: Option<User>,
    ) -> Result<Uuid> {
        if cfg!(windows) {
            ensure!(self.network_matches(), "The LAN is unavailable.");
        }
        invited.sort_unstable();
        invited.dedup();
        let own = &self.engine.own;
        let identity = self.engine.identity();
        let mut state = self.engine.state();
        if cfg!(windows) && !open && invited.len() == 1 {
            let peer = Some(invited[0]);
            if let Some(room) = state.hosted.values().map(|host| &host.room)
                .chain(state.guests.values().map(|guest| &guest.room))
                .find(|room| room.direct_peer(own.id) == peer)
            {
                return Ok(room.id);
            }
        }
        if nearby.is_none() && let Some(conversation) = conversation {
            if let Some(room) = state.guests.values().find(|guest| guest.room.conversation == Some(conversation)) {
                return Ok(room.room.id);
            }
            if let Some(room) = state.hosted.values().find(|host| host.room.conversation == Some(conversation)) {
                return Ok(room.room.id);
            }
        }
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
        let people = validate_room(&state.memberships, own, &room, nearby.as_ref())?;
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
                suspended_at: None,
                can_resume: false,
                nearby: nearby.is_some(),
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
            addresses: self.engine.lan()?.addresses().to_vec(),
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
            let Ok(connection) = self.engine.lan()?.connect(&identity, code.host, address).await
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
                let Wire::Offer { room, decided, can_resume } = recv_frame(&mut recv).await? else {
                    bail!("This QR session is unavailable.");
                };
                ensure!(
                    room.id == code.room && room.open,
                    "This QR code does not match the private session."
                );
                attach_guest_task(&self.engine, connection, send, recv, room, true, decided, can_resume)?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if let Ok(Ok(())) = result {
                return Ok(code.room);
            }
        }
        bail!("The private QR session is unavailable on this LAN.")
    }

    pub fn respond(&self, id: Uuid, accept: bool) -> Result<PrivateResponse> {
        let identity = self.engine.identity();
        let mut state = self.engine.state();
        let guest = state
            .guests
            .get_mut(&id)
            .context("This invitation is unavailable.")?;
        let conversation = guest.room.conversation;
        if guest.voted && guest.sender.is_some() && if accept {
            matches!(guest.summary.phase, Phase::Authenticating | Phase::Connected)
        } else {
            guest.summary.phase == Phase::Waiting
        } {
            return Ok(PrivateResponse { conversation, applied: false });
        }
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
            .try_send(Wire::Decision { approval, can_resume: guest.can_resume })?;
        guest.voted = true;
        guest.summary.phase = if accept {
            Phase::Authenticating
        } else {
            Phase::Waiting
        };
        Ok(PrivateResponse { conversation, applied: true })
    }

    pub fn send(&self, id: Uuid, text: String) -> Result<()> {
        if cfg!(windows) {
            ensure!(self.network_matches(), "The LAN is unavailable.");
        }
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
                guest.summary.phase == Phase::Connected
                    && (!cfg!(windows) || guest.transport.as_ref().is_some_and(|connection| connection.close_reason().is_none())),
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
        } else {
            state.end_guest(id);
        }
    }

    pub fn shutdown(&self) {
        self.stop(0);
    }

    pub async fn quit(&self) {
        let lan = self.engine.lan().ok();
        self.stop(APP_EXIT);
        if let Some(lan) = lan {
            let _ = timeout(Duration::from_millis(500), async {
                tokio::join!(lan.announce_departure(), lan.endpoint.wait_idle());
            }).await;
        }
    }

    fn stop(&self, code: u32) {
        self.stop_transport(code);
        let mut state = self.engine.state();
        #[cfg(target_os = "ios")]
        for &room in state.guests.keys() {
            state.dismissed.record(room);
        }
        state.hosted.clear();
        state.guests.clear();
    }

    fn stop_transport(&self, code: u32) {
        if let Some(task) = self.task.lock().expect("LAN worker").take() {
            task.abort();
        }
        if let Some(lan) = self.engine.lan.lock().expect("LAN transport").take() {
            lan.endpoint.close(code.into(), b"");
        }
    }

    #[cfg(windows)]
    pub fn suspend_transport(&self) {
        self.stop_transport(0);
        let mut state = self.engine.state();
        state.hosted.retain(|_, host| {
            if !host.can_resume || !host.ever_connected { return false; }
            host.links.clear();
            host.attempts.clear();
            host.suspended_at.get_or_insert_with(Instant::now).elapsed() < ROOM_IDLE
        });
        state.guests.retain(|_, guest| {
            if !guest.can_resume || !matches!(guest.summary.phase,
                Phase::Connected | Phase::Suspended | Phase::Reconnecting) { return false; }
            guest.sender = None;
            guest.transport = None;
            guest.summary.phase = Phase::Suspended;
            guest.suspended_at.get_or_insert_with(Instant::now).elapsed() < ROOM_IDLE
        });
    }

    #[cfg(windows)]
    pub fn restore_transport(&self, origin: String) -> Result<()> {
        let mut task = self.task.lock().expect("LAN worker");
        let mut transport = self.engine.lan.lock().expect("LAN transport");
        if transport.is_some() { return Ok(()); }
        let lan = Arc::new(Lan::new(Scope::current()?, &self.engine.identity(), origin, self.engine.own.id)?);
        *transport = Some(lan.clone());
        *task = Some(tokio::spawn(background(self.engine.clone(), lan)));
        Ok(())
    }
}

impl Guest {
    fn summary(&self, own: &User) -> PrivateSummary {
        let mut summary = self.summary.clone();
        summary.unread = (summary.phase == Phase::Connected || self.can_resume) && self.feed.unread(own.id);
        summary
    }
}

fn restore_room_names(
    room: &mut PrivateSummary,
    memberships: &HashMap<Uuid, Vec<User>>,
    own: &User,
) {
    let members = room
        .conversation
        .and_then(|id| memberships.get(&id))
        .map(Vec::as_slice);
    for user in std::iter::once(&mut room.host)
        .chain(&mut room.invited)
        .chain(&mut room.participants)
    {
        restore_name(user, members, own);
    }
}

fn restore_name(user: &mut User, members: Option<&[User]>, own: &User) {
    let matching = if user.id == own.id {
        Some(own)
    } else {
        members.and_then(|members| members.iter().find(|member| member.id == user.id))
    };
    if let Some(matching) = matching {
        user.handle.clone_from(&matching.handle);
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
    own: &User,
    room: &Room,
    peer: Option<&User>,
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
        own.id == room.host || room.invited.contains(&own.id),
        "This invitation belongs to another account."
    );
    if room.standalone() {
        let peer = peer.context("Authenticate the selected LAN device first.")?;
        let invitee = if own.id == room.host { peer } else { own };
        ensure!(
            room.invited == [invitee.id],
            "A nearby invitation is for one account."
        );
        return Ok(vec![invitee.clone()]);
    }
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

async fn background(engine: Arc<Engine>, lan: Arc<Lan>) {
    let mut tick = interval(Duration::from_secs(2));
    let connections = Arc::new(Semaphore::new(128));
    let mut discovery = None::<JoinHandle<()>>;
    loop {
        tokio::select! {
            incoming = lan.endpoint.accept() => {
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
                    discovery = Some(tokio::spawn(discover(engine.clone(), lan.clone())));
                }
            }
        }
    }
}

async fn discover(engine: Arc<Engine>, lan: Arc<Lan>) {
    let accounts: Vec<_> = {
        let mut state = engine.state();
        state.guests.retain(|_, guest| guest.suspended_at.is_none_or(|since| since.elapsed() < ROOM_IDLE));
        state.hosted.retain(|_, host| {
            let limit = if host.ever_connected || host.room.open {
                ROOM_IDLE
            } else {
                INVITATION_TIMEOUT
            };
            let live = if host.can_resume && host.ever_connected {
                host.suspended_at.is_none_or(|since| since.elapsed() < ROOM_IDLE)
            } else {
                host.feed.last_text.elapsed() < limit
            };
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
            .filter(|host| !host.nearby || (host.can_resume && host.ever_connected))
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
    let peers = lan.candidates(&accounts).await;
    let mut attempts = Vec::new();
    {
        let mut state = engine.state();
        if !engine.lan().is_ok_and(|current| Arc::ptr_eq(&current, &lan)) { return; }
        for host in state.hosted.values_mut().filter(|host|
            (!host.nearby || (host.can_resume && host.ever_connected))
                && (!host.can_resume || !host.has_peer(engine.own.id))) {
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
        let lan = lan.clone();
        tokio::spawn(async move {
            let _ = outbound(&engine, &lan, room, account, address).await;
            if engine.lan().is_ok_and(|current| Arc::ptr_eq(&current, &lan))
                && let Some(host) = engine.state().hosted.get_mut(&room) {
                host.attempts.remove(&address);
            }
        });
    }
}

async fn outbound(
    engine: &Arc<Engine>,
    lan: &Lan,
    room: Uuid,
    account: Uuid,
    address: SocketAddr,
) -> Result<()> {
    let connection = lan
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
        Wire::Offer { room, decided, can_resume } => {
            let task = attach_guest_task(&engine, connection, send, recv, room, false, decided, can_resume)?;
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
        ensure!(connection.close_reason().is_none(), "The device disconnected.");
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
        if host.can_resume && let Some(approval) = approval.as_ref() {
            ensure!(approval.device()? == device, "Resume on the device that approved the session.");
        }
        sender.try_send(Wire::Offer {
            room: host.room.clone(),
            decided: approval.is_some(),
            can_resume: if approval.is_some() { host.can_resume } else { host.room.resumable() },
        })?;
        host.links.insert(
            device,
            Link {
                sender,
                user: peer.user,
                active: approval.is_some() && !host.can_resume,
                connection: connection_id,
                transport: connection.clone(),
            },
        );
        if let Some(approval) = approval.filter(|_| !host.can_resume) {
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
            Wire::Decision { approval, can_resume } if !host.decisions.contains_key(&account)
                || (host.can_resume && !link.active
                    && host.decisions.get(&account).is_some_and(Option::is_some)) => {
                if let Some(approval) = approval {
                    if let Some(secret) = qr_secret {
                        ensure!(
                            host.valid_qr(secret, Instant::now()),
                            "The private QR code expired."
                        );
                    }
                    identity.verify(&approval, &host.room.proof_message()?, account, true)?;
                    let resume = host.room.resumable() && can_resume;
                    ensure!(!host.can_resume || resume, "The accepting device did not confirm session continuity.");
                    if resume {
                        ensure!(approval.device()? == device, "Approval belongs to another device.");
                    }
                    host.can_resume = resume;
                    if !host.decisions.contains_key(&account) {
                        let count = host.decisions.values().filter(|entry| entry.is_some()).count();
                        ensure!(count < MAX_DEVICE_COUNT, "This private group is full.");
                    }
                    host.decisions.insert(account, Some(approval.clone()));
                    for link in host
                        .links
                        .values_mut()
                        .filter(|link| link.user.id == account
                            && (!resume || link.connection == connection_id))
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
            Wire::Leave => {
                reject_account(host, &engine.own, account);
                if host.all_declined() && let Some(host) = state.hosted.remove(&id) {
                    host.close();
                }
            }
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
    if account != own.id {
        host.suspended_at = None;
    }
    let approving_device = host.can_resume.then(|| approval.device().ok()).flatten();
    let participants = host.wire_participants(own);
    host.links.retain(|device, link| {
        link.user.id != account
            || (approving_device.is_none_or(|expected| expected == *device) && link
                .sender
                .try_send(Wire::Joined {
                    approval: approval.clone(),
                    participants: participants.clone(),
                })
                .is_ok())
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
    can_resume: bool,
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
        ensure!(connection.close_reason().is_none(), "The device disconnected.");
        let named = validate_room(&state.memberships, &engine.own, &room, Some(&peer.user))?;
        if let Some(account) = room.direct_peer(engine.own.id) {
            let conflict = |other: &Room| other.id != id && other.direct_peer(engine.own.id) == Some(account);
            if let Some(host) = state.hosted.values().find(|host| conflict(&host.room)) {
                ensure!(!host.ever_connected && id < host.room.id, "A private session with this contact already exists.");
                let old = host.room.id;
                if let Some(host) = state.hosted.remove(&old) { host.close(); }
            }
            if let Some(guest) = state.guests.values().find(|guest| conflict(&guest.room)) {
                ensure!(guest.summary.phase == Phase::Invited && id < guest.room.id,
                    "A private session with this contact already exists.");
                let old = guest.room.id;
                state.end_guest(old);
            }
        }
        let can_resume = room.resumable() && can_resume;
        let resuming = can_resume && decided && state.guests.get(&id).is_some_and(|old|
            old.room == room && old.sender.is_none() && old.summary.phase == Phase::Suspended
                && old.suspended_at.is_some_and(|since| since.elapsed() < ROOM_IDLE));
        ensure!(
            !decided || resuming || (!room.standalone() && !can_resume),
            "A nearby invitation requires fresh consent."
        );
        if let Some(old) = state.guests.get(&id) {
            ensure!(
                old.room == room && old.sender.is_none() && (!can_resume || resuming),
                "A private connection already exists."
            );
        }
        if state.guests.len() >= MAX_ROOM_COUNT {
            state.guests.retain(|_, guest| guest.sender.is_some() || guest.suspended_at.is_some());
        }
        ensure!(
            resuming || state.guests.len() < MAX_ROOM_COUNT,
            "Too many private invitations."
        );
        if scanned || resuming {
            let approval = engine.identity().sign(&room.proof_message()?)?;
            sender.try_send(Wire::Decision {
                approval: Some(approval),
                can_resume,
            })?;
        }
        state.dismissed.admit(id, scanned)?;
        let phase = if resuming {
            Phase::Reconnecting
        } else if decided || room.open || room.host == engine.own.id || scanned {
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
            unread: false,
            open: room.open,
            hosted: false,
        };
        let previous = if resuming { state.guests.remove(&id) } else { None };
        let suspended_at = previous.as_ref().and_then(|guest| guest.suspended_at);
        state.guests.insert(
            id,
            Guest {
                room,
                summary,
                link,
                sender: Some(sender),
                transport: Some(connection.clone()),
                voted: scanned || resuming,
                feed: previous.map_or_else(Feed::new, |guest| guest.feed),
                suspended_at,
                can_resume,
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
                .filter(|guest| guest.link == link && guest.sender.is_some())
                .context("The private session ended.")?;
            match frame {
                Wire::Joined {
                    approval,
                    participants,
                } => {
                    ensure!(
                        !guest.room.standalone()
                            || (guest.voted && matches!(guest.summary.phase, Phase::Authenticating | Phase::Reconnecting)),
                        "A nearby invitation requires fresh consent."
                    );
                    identity.verify(
                        &approval,
                        &guest.room.proof_message()?,
                        engine.own.id,
                        false,
                    )?;
                    if guest.can_resume {
                        ensure!(approval.device()? == identity.device, "Approval belongs to another device.");
                    }
                    ensure!(
                        valid_roster(&guest.room, &participants, engine.own.id),
                        "Invalid private roster."
                    );
                    guest.summary.phase = Phase::Connected;
                    guest.summary.participants = participants;
                    guest.suspended_at = None;
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
        let mut state = engine.state();
        if let Some(guest) = state.guests.get_mut(&id).filter(|guest| guest.link == link && guest.sender.is_some()) {
            let quit = matches!(connection.close_reason(), Some(ConnectionError::ApplicationClosed(close))
                if close.error_code.into_inner() == u64::from(APP_EXIT));
            if !quit && guest.can_resume && matches!(guest.summary.phase, Phase::Connected | Phase::Reconnecting) {
                guest.summary.phase = Phase::Suspended;
                guest.sender = None;
                guest.transport = None;
                guest.suspended_at.get_or_insert_with(Instant::now);
                return;
            }
            if quit || matches!(guest.summary.phase, Phase::Ended | Phase::Missed) {
                state.dismissed.record(id);
            }
            state.guests.remove(&id);
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
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose,
    };

    #[test]
    fn dismissing_a_disconnected_guest_blocks_automatic_rejoin_after_rebind() {
        let host = User {
            id: Uuid::new_v4(),
            handle: "host".into(),
        };
        let room = Room {
            id: Uuid::new_v4(),
            conversation: None,
            host: host.id,
            host_device: [0; 32],
            invited: Vec::new(),
            open: true,
        };
        let id = room.id;
        let mut state = State::default();
        state.guests.insert(
            id,
            Guest {
                room,
                summary: PrivateSummary {
                    id,
                    conversation: None,
                    host,
                    invited: Vec::new(),
                    participants: Vec::new(),
                    phase: Phase::Failed,
                    unread: false,
                    open: true,
                    hosted: false,
                },
                link: Uuid::new_v4(),
                sender: None,
                transport: None,
                voted: true,
                feed: Feed::new(),
                suspended_at: None,
                can_resume: false,
            },
        );
        state.end_guest(id);
        assert!(!state.guests.contains_key(&id));
        let rebound = State {
            dismissed: state.dismissed.clone(),
            ..State::default()
        };
        assert!(rebound.dismissed.admit(id, false).is_err());
        assert!(rebound.dismissed.admit(Uuid::new_v4(), false).is_ok());
        assert!(rebound.dismissed.admit(id, true).is_ok());
        assert!(rebound.dismissed.admit(id, false).is_ok());
    }

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
            suspended_at: None,
            can_resume: false,
            nearby: false,
        };
        assert!(host.valid_qr(secret, deadline - Duration::from_millis(1)));
        assert!(!host.valid_qr(secret, deadline));
    }

    #[tokio::test]
    #[ignore = "requires a private IPv4 interface with local mDNS and UDP"]
    async fn nearby_requires_fresh_consent_from_certified_strangers() -> Result<()> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let authority = params.self_signed(&key)?;
        let issuer = rcgen::Issuer::new(params, key);
        let start = |handle: &str| -> Result<PrivateLobby> {
            let own = User {
                id: Uuid::new_v4(),
                handle: handle.into(),
            };
            let key = DeviceKey::generate()?;
            let mut params =
                CertificateParams::new(vec![lan_peer_name(own.id), LAN_NEARBY_NAME.into()])?;
            params
                .distinguished_name
                .push(rcgen::DnType::CommonName, handle);
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            params.extended_key_usages = vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ];
            let certificate = params.signed_by(key.key_pair(), &issuer)?;
            PrivateLobby::new(
                "https://zibblu-test.invalid/".into(),
                own,
                key,
                LanCredential {
                    certificate: certificate.der().to_vec(),
                    authority: authority.der().to_vec(),
                    expires_at: i64::MAX,
                },
                HashMap::new(),
                DismissedRooms::default(),
            )
        };
        let alice = start("alice")?;
        let bob = start("bob")?;
        let expected = discovery_instance(&bob.engine.identity().device);
        let device = timeout(Duration::from_secs(12), async {
            loop {
                if let Some(device) = alice
                    .nearby_devices()
                    .await
                    .devices
                    .into_iter()
                    .find(|device| device.id == expected && device.matched_account.is_none())
                {
                    break device.id;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("The local mDNS environment did not discover the second test device")?;
        assert!(
            alice
                .invite_nearby(device, Some(Uuid::new_v4()))
                .await
                .is_err()
        );
        let id = alice.invite_nearby(device, None).await?;
        let wait_phase = |lobby: &PrivateLobby, phase: Phase| {
            let engine = lobby.engine.clone();
            async move {
                timeout(Duration::from_secs(5), async {
                    loop {
                        if engine
                            .state()
                            .guests
                            .get(&id)
                            .is_some_and(|guest| guest.summary.phase == phase)
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
            }
        };
        wait_phase(&bob, Phase::Invited).await?;
        assert!(alice.send(id, "Before consent".into()).is_err());
        assert!(bob.send(id, "Before consent".into()).is_err());
        assert!(bob.respond(id, true)?.applied);
        assert!(!bob.respond(id, true)?.applied);
        wait_phase(&bob, Phase::Connected).await?;
        alice.send(id, "LAN message".into())?;
        timeout(Duration::from_secs(5), async {
            loop {
                if bob.view(id, 0).is_ok_and(|view| {
                    view.messages
                        .iter()
                        .any(|message| message.text == "LAN message")
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let view = bob.view(id, 0)?;
        assert!(view.summary.unread);
        assert!(!bob.mark_read(id, Uuid::new_v4(), view.messages[0].seq));
        assert!(bob.mark_read(id, view.connection, view.messages[0].seq));
        assert!(!bob.view(id, 0)?.summary.unread);
        assert!(!bob.respond(id, true)?.applied);
        alice.end(id);
        timeout(Duration::from_secs(5), async {
            while bob.view(id, 0).is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await?;
        assert!(!bob.list().iter().any(|room| room.id == id));
        assert!(bob.respond(id, true).is_err());
        let declined = alice.invite_nearby(device, None).await?;
        timeout(Duration::from_secs(5), async {
            while !bob
                .list()
                .iter()
                .any(|room| room.id == declined && room.phase == Phase::Invited)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        bob.respond(declined, false)?;
        assert!(bob.send(declined, "After declining".into()).is_err());
        alice.quit().await;
        bob.quit().await;
        Ok(())
    }
}
