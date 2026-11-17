use super::identity::Identity;
use super::DiscoveryStatus;
use crate::lan_peer_name;
use anyhow::{Context, Result, ensure};
use hickory_resolver::{
    ResolveError, TokioResolver,
    proto::rr::{RData, RecordType},
};
use if_addrs::{IfAddr, Interface};
use ipnet::Ipv4Net;
#[cfg(not(target_os = "ios"))]
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, Runtime, TokioRuntime, TransportConfig, UdpPoller,
    udp::{RecvMeta, Transmit},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, ready},
    time::Duration,
};
use tokio::time::timeout;
use uuid::Uuid;

#[cfg(not(target_os = "ios"))]
use {std::net::SocketAddrV4, tokio::task::JoinHandle};

#[cfg(target_os = "ios")]
mod bonjour;

const SERVICE: &str = "_zibblu-private._udp.local.";
const ATTEMPT: Duration = Duration::from_secs(3);
type Records = Arc<Mutex<HashMap<String, (String, Vec<SocketAddr>)>>>;

pub(super) struct NearbyRecord {
    pub tag: String,
    pub addresses: HashSet<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scope {
    interfaces: Vec<Interface>,
    enterprise: Vec<Ipv4Net>,
    dns_domain: Option<String>,
    port: u16,
    #[cfg(target_os = "ios")]
    wifi: u64,
}

#[derive(Debug)]
pub(super) struct NoLanInterface;

impl std::fmt::Display for NoLanInterface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("No approved IPv4 LAN interface is available.")
    }
}

impl std::error::Error for NoLanInterface {}

impl Scope {
    pub fn current() -> Result<Self> {
        #[cfg(target_os = "ios")]
        let wifi = bonjour::wifi();
        #[cfg(target_os = "ios")]
        ensure!(wifi as u32 != 0, NoLanInterface);
        let enterprise = match std::env::var("ZIBBLU_LAN_SITE_SUBNETS") {
            Ok(value) => value
                .split(',')
                .map(|cidr| {
                    let net: Ipv4Net =
                        cidr.trim().parse().context("Invalid company LAN subnet.")?;
                    ensure!(
                        net.network().is_private() && net.broadcast().is_private(),
                        "Company LAN subnets must be private IPv4 ranges."
                    );
                    Ok(net)
                })
                .collect::<Result<Vec<_>>>()?,
            Err(std::env::VarError::NotPresent) => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let port = match std::env::var("ZIBBLU_LAN_PORT") {
            Ok(value) => {
                let port = value.parse::<u16>().context("Invalid company LAN port.")?;
                ensure!(
                    !enterprise.is_empty() && port != 0,
                    "Set a valid company LAN port."
                );
                port
            }
            Err(std::env::VarError::NotPresent) => 0,
            Err(error) => return Err(error.into()),
        };
        let dns_domain = std::env::var("ZIBBLU_LAN_DISCOVERY_DOMAIN")
            .ok()
            .map(|domain| {
                let domain = domain.trim().trim_end_matches('.');
                ensure!(
                    !enterprise.is_empty()
                        && domain.len() <= 230
                        && domain.contains('.')
                        && domain.split('.').all(|label| !label.is_empty()
                            && label.len() <= 63
                            && label
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')),
                    "Set a valid company DNS-SD discovery domain."
                );
                Ok(domain.to_owned())
            })
            .transpose()?;
        let mut interfaces: Vec<_> = if_addrs::get_if_addrs()?
            .into_iter()
            .filter(|interface| interface.is_oper_up() && !interface.is_p2p())
            .filter(|_interface| {
                #[cfg(target_os = "ios")]
                return _interface.index == Some(wifi as u32);
                #[cfg(not(target_os = "ios"))]
                true
            })
            .filter(|interface| {
                let IfAddr::V4(ip) = &interface.addr else {
                    return false;
                };
                if !(1..=31).contains(&ip.prefixlen) {
                    return false;
                }
                if enterprise.is_empty() {
                    ip.ip.is_private() || ip.ip.is_link_local()
                } else {
                    enterprise.iter().any(|net| net.contains(&ip.ip))
                }
            })
            .collect();
        interfaces.sort_by_key(|interface| (interface.ip(), interface.index));
        ensure!(!interfaces.is_empty(), NoLanInterface);
        Ok(Self {
            interfaces,
            enterprise,
            dns_domain,
            port,
            #[cfg(target_os = "ios")]
            wifi,
        })
    }

    fn allows(&self, peer: SocketAddr) -> bool {
        #[cfg(target_os = "ios")]
        if self.wifi != bonjour::wifi() {
            return false;
        }
        let SocketAddr::V4(peer) = peer else {
            return false;
        };
        if peer.port() == 0 {
            return false;
        }
        let ip = *peer.ip();
        if !self.enterprise.is_empty() {
            return self.enterprise.iter().any(|net| {
                net.contains(&ip)
                    && (net.prefix_len() >= 31 || (ip != net.network() && ip != net.broadcast()))
            });
        }
        if !ip.is_private() && !ip.is_link_local() {
            return false;
        }
        self.interfaces.iter().any(|local| {
            let IfAddr::V4(local) = &local.addr else {
                return false;
            };
            let net = Ipv4Net::new(local.ip, local.prefixlen).expect("IPv4 interface prefix");
            net.contains(&ip)
                && (net.prefix_len() >= 31 || (ip != net.network() && ip != net.broadcast()))
        })
    }
}

pub(super) struct Lan {
    pub endpoint: Endpoint,
    pub scope: Scope,
    origin: String,
    bound: Vec<SocketAddr>,
    records: Records,
    #[cfg(not(target_os = "ios"))]
    daemon: ServiceDaemon,
    service_name: String,
    #[cfg(not(target_os = "ios"))]
    task: JoinHandle<()>,
    #[cfg(target_os = "ios")]
    bonjour: bonjour::Bonjour,
    resolver: Option<TokioResolver>,
}

impl Lan {
    pub fn new(scope: Scope, identity: &Identity, origin: String, account: Uuid) -> Result<Self> {
        let bind = if scope.enterprise.is_empty() {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            scope.interfaces[0].ip()
        };
        let socket = UdpSocket::bind((bind, scope.port))?;
        #[cfg(target_os = "ios")]
        bonjour::bind_to_wifi(&socket, scope.wifi as u32)?;
        socket.set_ttl(if scope.enterprise.is_empty() { 1 } else { 16 })?;
        let runtime = Arc::new(TokioRuntime);
        let udp = Arc::new(LanSocket {
            udp: runtime.wrap_udp_socket(socket)?,
            scope: scope.clone(),
        });
        let endpoint = Endpoint::new_with_abstract_socket(
            Default::default(),
            Some(server_config(identity)),
            udp,
            runtime,
        )?;
        let port = endpoint.local_addr()?.port();
        let addresses: Vec<_> = scope
            .interfaces
            .iter()
            .map(Interface::ip)
            .filter(|&ip| bind.is_unspecified() || ip == bind)
            .collect();
        let bound = addresses
            .iter()
            .map(|ip| SocketAddr::new(*ip, port))
            .collect();

        let resolver = scope
            .dns_domain
            .as_ref()
            .map(|_| TokioResolver::builder_tokio().map(|builder| builder.build()))
            .transpose()?;
        let device = discovery_instance(&identity.device).simple().to_string();
        let records = Records::default();
        #[cfg(target_os = "ios")]
        let (bonjour, service_name) = (
            bonjour::Bonjour::new(&device, &account_tag(&origin, account), port, scope.wifi),
            format!("{device}.{SERVICE}"),
        );
        #[cfg(not(target_os = "ios"))]
        let (daemon, task, service_name) = {
        let daemon = ServiceDaemon::new()?;
        let props = [("account", account_tag(&origin, account))];
        let info = ServiceInfo::new(
            SERVICE,
            &device,
            &format!("{device}.local."),
            addresses.as_slice(),
            port,
            props.as_slice(),
        )?;
        let service_name = info.get_fullname().to_owned();
        let setup = (|| {
            daemon.disable_interface(IfKind::All)?;
            daemon.enable_interface(addresses)?;
            let events = daemon.browse(SERVICE)?;
            daemon.register(info)?;
            Ok::<_, mdns_sd::Error>(events)
        })();
        let events = match setup {
            Ok(events) => events,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error.into());
            }
        };
        let discovered = records.clone();
        let task = tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let mut discovered = discovered.lock().expect("LAN discovery records");
                        let Some(tag) = info
                            .get_property_val_str("account")
                            .filter(|tag| valid_tag(tag))
                        else {
                            discovered.remove(info.get_fullname());
                            continue;
                        };
                        let addresses = info
                            .get_addresses_v4()
                            .into_iter()
                            .map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, info.get_port())))
                            .collect();
                        if discovered.len() < 512 || discovered.contains_key(info.get_fullname()) {
                            discovered.insert(
                                info.get_fullname().to_owned(),
                                (tag.to_owned(), addresses),
                            );
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, name) => {
                        discovered
                            .lock()
                            .expect("LAN discovery records")
                            .remove(&name);
                    }
                    _ => {}
                }
            }
        });
        (daemon, task, service_name)
        };
        Ok(Self {
            endpoint,
            scope,
            origin,
            bound,
            records,
            #[cfg(not(target_os = "ios"))]
            daemon,
            service_name,
            #[cfg(not(target_os = "ios"))]
            task,
            #[cfg(target_os = "ios")]
            bonjour,
            resolver,
        })
    }

    pub fn addresses(&self) -> &[SocketAddr] {
        &self.bound
    }

    pub fn set_identity(&self, identity: &Identity) {
        self.endpoint
            .set_server_config(Some(server_config(identity)));
    }

    pub fn discovery_status(&self) -> DiscoveryStatus {
        #[cfg(target_os = "ios")]
        return self.bonjour.status();
        #[cfg(not(target_os = "ios"))]
        if self.task.is_finished() {
            DiscoveryStatus::DiscoveryFailed
        } else {
            DiscoveryStatus::Ready
        }
    }

    pub async fn announce_departure(&self) {
        #[cfg(target_os = "ios")]
        self.bonjour.stop();
        #[cfg(not(target_os = "ios"))]
        if let Ok(unregistered) = self.daemon.unregister(&self.service_name) {
            let _ = unregistered.recv_async().await;
        }
    }

    pub fn tag(&self, account: Uuid) -> String {
        account_tag(&self.origin, account)
    }

    pub async fn nearby(&self, selected: Option<Uuid>) -> Result<HashMap<Uuid, NearbyRecord>> {
        #[cfg(target_os = "ios")]
        self.bonjour.refresh(&self.records);
        let own = service_instance(&self.service_name, SERVICE);
        let device = |name: &str, service: &str| {
            service_instance(name, service)
                .filter(|id| Some(*id) != own && selected.is_none_or(|selected| selected == *id))
        };
        let mut site = Ok(Vec::new());
        if let (Some(resolver), Some(domain)) = (&self.resolver, &self.scope.dns_domain) {
            let service = site_service(domain);
            let identify =
                |name: &str, tag: &str| device(name, &service).map(|id| (id, tag.to_owned()));
            site = match timeout(ATTEMPT, site_records(resolver, &service, identify)).await {
                Ok(Ok(entries)) => Ok(entries),
                Ok(Err(error))
                    if error
                        .downcast_ref::<ResolveError>()
                        .is_some_and(ResolveError::is_no_records_found) =>
                {
                    Ok(Vec::new())
                }
                _ => Err(anyhow::anyhow!("Site discovery failed.")),
            };
        }
        let mut nearby = HashMap::new();
        let mut add = |id, tag: &str, address| {
            if self.is_peer(address) {
                let entry = nearby.entry(id).or_insert_with(|| NearbyRecord {
                    tag: tag.to_owned(),
                    addresses: HashSet::new(),
                });
                if entry.tag == tag {
                    entry.addresses.insert(address);
                }
            }
        };
        {
            let records = self.records.lock().expect("LAN discovery records");
            for (name, (tag, addresses)) in records.iter() {
                if let Some(id) = device(name, SERVICE) {
                    for &address in addresses {
                        add(id, tag, address);
                    }
                }
            }
        }
        match site {
            Ok(entries) => {
                for ((id, tag), address) in entries {
                    add(id, &tag, address);
                }
            }
            Err(error) if nearby.is_empty() => return Err(error),
            Err(_) => {}
        }
        Ok(nearby)
    }

    fn is_peer(&self, address: SocketAddr) -> bool {
        self.scope.allows(address) && !self.bound.contains(&address)
    }

    pub async fn candidates(&self, accounts: &[Uuid]) -> Vec<(Uuid, SocketAddr)> {
        #[cfg(target_os = "ios")]
        self.bonjour.refresh(&self.records);
        let tags: HashMap<_, _> = accounts
            .iter()
            .map(|&account| (self.tag(account), account))
            .collect();
        let mut peers = HashSet::new();
        {
            let records = self.records.lock().expect("LAN discovery records");
            for (tag, addresses) in records.values() {
                if let Some(&account) = tags.get(tag) {
                    peers.extend(addresses.iter().copied().map(|address| (account, address)));
                }
            }
        }
        if let (Some(resolver), Some(domain)) = (&self.resolver, &self.scope.dns_domain) {
            let service = site_service(domain);
            let identify = |_: &str, tag: &str| tags.get(tag).copied();
            if let Ok(Ok(entries)) =
                timeout(ATTEMPT, site_records(resolver, &service, identify)).await
            {
                peers.extend(entries);
            }
        }
        peers
            .into_iter()
            .filter(|(_, addr)| self.is_peer(*addr))
            .collect()
    }

    pub async fn connect(
        &self,
        identity: &Identity,
        account: Uuid,
        address: SocketAddr,
    ) -> Result<Connection> {
        self.connect_name(identity, &lan_peer_name(account), address)
            .await
    }

    pub async fn connect_name(
        &self,
        identity: &Identity,
        name: &str,
        address: SocketAddr,
    ) -> Result<Connection> {
        ensure!(
            self.scope.allows(address),
            "This device is outside the allowed LAN."
        );
        let mut client = identity.client.clone();
        client.transport_config(transport());
        let connection = self.endpoint.connect_with(client, address, name)?;
        Ok(timeout(ATTEMPT, connection).await??)
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.endpoint.close(0_u32.into(), b"");
        #[cfg(not(target_os = "ios"))]
        {
        self.task.abort();
        let _ = self.daemon.unregister(&self.service_name);
        let _ = self.daemon.shutdown();
        }
    }
}

fn account_tag(origin: &str, account: Uuid) -> String {
    format!(
        "{:x}",
        Sha256::new()
            .chain_update(origin)
            .chain_update(account.as_bytes())
            .finalize()
    )
}

fn service_instance(name: &str, service: &str) -> Option<Uuid> {
    Uuid::parse_str(name.strip_suffix(service)?.strip_suffix('.')?).ok()
}

pub(super) fn discovery_instance(fingerprint: &[u8; 32]) -> Uuid {
    Uuid::from_slice(&fingerprint[..16]).expect("16-byte discovery instance")
}

fn valid_tag(tag: &str) -> bool {
    tag.len() == 64 && tag.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn site_service(domain: &str) -> String {
    format!("_zibblu-private._udp.{domain}.").to_ascii_lowercase()
}

async fn site_records<K: Clone>(
    resolver: &TokioResolver,
    service: &str,
    identify: impl Fn(&str, &str) -> Option<K>,
) -> Result<Vec<(K, SocketAddr)>> {
    let records = resolver.lookup(service, RecordType::PTR).await?;
    let mut addresses = Vec::new();
    for record in records.iter().take(128) {
        let RData::PTR(ptr) = record else { continue };
        let Ok(txt) = resolver.txt_lookup(ptr.0.clone()).await else {
            continue;
        };
        let name = ptr.0.to_ascii().to_ascii_lowercase();
        let device = txt
            .iter()
            .flat_map(|txt| txt.txt_data())
            .filter_map(|field| std::str::from_utf8(field).ok()?.strip_prefix("account="))
            .filter(|tag| valid_tag(tag))
            .find_map(|tag| identify(&name, tag));
        let Some(device) = device else { continue };
        let Ok(services) = resolver.srv_lookup(ptr.0.clone()).await else {
            continue;
        };
        for service in services.iter() {
            let Ok(ips) = resolver.ipv4_lookup(service.target().clone()).await else {
                continue;
            };
            addresses.extend(
                ips.iter()
                    .map(|ip| (device.clone(), SocketAddr::from((ip.0, service.port())))),
            );
        }
    }
    Ok(addresses)
}

fn transport() -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(1_u32.into())
        .max_concurrent_uni_streams(0_u32.into())
        .datagram_receive_buffer_size(None)
        .max_idle_timeout(Some(
            Duration::from_secs(12)
                .try_into()
                .expect("QUIC idle timeout"),
        ))
        .keep_alive_interval(Some(Duration::from_secs(3)));
    Arc::new(transport)
}

fn server_config(identity: &Identity) -> quinn::ServerConfig {
    let mut config = identity.server.clone();
    config.migration(false);
    config.max_incoming(16);
    config.transport_config(transport());
    config
}

#[derive(Debug)]
struct LanSocket {
    udp: Arc<dyn AsyncUdpSocket>,
    scope: Scope,
}

impl AsyncUdpSocket for LanSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.udp.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if !self.scope.allows(transmit.destination) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        self.udp.try_send(transmit)
    }

    fn poll_recv(
        &self,
        cx: &mut TaskContext<'_>,
        buffers: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let count = ready!(self.udp.poll_recv(cx, &mut buffers[..1], &mut meta[..1]))?;
        if count != 0 && !self.scope.allows(meta[0].addr) {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(Ok(count))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    fn max_receive_segments(&self) -> usize {
        self.udp.max_receive_segments()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use if_addrs::{IfOperStatus, Ifv4Addr};

    fn interface(index: u32, ip: Ipv4Addr) -> Interface {
        Interface {
            name: format!("LAN {index}"),
            addr: IfAddr::V4(Ifv4Addr {
                ip,
                netmask: "255.255.255.0".parse().unwrap(),
                prefixlen: 24,
                broadcast: None,
            }),
            index: Some(index),
            oper_status: IfOperStatus::Up,
            is_p2p: false,
            #[cfg(windows)]
            adapter_name: format!("adapter-{index}"),
        }
    }

    #[test]
    fn nearby_identifies_service_instances_independently_of_account_tags() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let service = site_service("Site.Example");
        assert_eq!(
            service_instance(&format!("{}.{SERVICE}", first.simple()), SERVICE),
            Some(first)
        );
        assert_eq!(
            service_instance(&format!("{}.{service}", first.simple()), &service),
            Some(first)
        );
        assert_eq!(
            service_instance(&format!("{}.{SERVICE}", second.simple()), SERVICE),
            Some(second)
        );
        assert_eq!(service_instance("printer._ipp._tcp.local.", SERVICE), None);
        assert!(valid_tag(&account_tag(
            "https://zibblu.example",
            Uuid::new_v4()
        )));
        assert!(!valid_tag(&"x".repeat(64)));
    }

    #[test]
    fn ordinary_and_company_networks_fail_closed() {
        let mut scope = Scope {
            interfaces: vec![interface(1, "192.168.20.2".parse().unwrap())],
            enterprise: Vec::new(),
            dns_domain: None,
            port: 0,
            #[cfg(target_os = "ios")]
            wifi: bonjour::wifi(),
        };
        assert!(scope.allows("192.168.20.3:1234".parse().unwrap()));
        assert!(!scope.allows("192.168.21.3:1234".parse().unwrap()));
        scope.enterprise = vec![
            "192.168.20.0/24".parse().unwrap(),
            "192.168.21.0/24".parse().unwrap(),
        ];
        assert!(scope.allows("192.168.21.3:1234".parse().unwrap()));
        for address in [
            "192.168.20.0:1234",
            "192.168.20.255:1234",
            "192.168.22.3:1234",
            "127.0.0.1:1234",
            "8.8.8.8:1234",
            "[::1]:1234",
        ] {
            assert!(!scope.allows(address.parse().unwrap()));
        }
    }

    #[test]
    fn switching_interfaces_invalidates_discovery_even_when_the_address_is_reused() {
        let ip = "192.168.20.2".parse().unwrap();
        let ethernet = Scope {
            interfaces: vec![interface(1, ip)],
            enterprise: Vec::new(),
            dns_domain: None,
            port: 0,
            #[cfg(target_os = "ios")]
            wifi: bonjour::wifi(),
        };
        let wifi = Scope {
            interfaces: vec![interface(2, ip)],
            ..ethernet.clone()
        };
        assert_ne!(ethernet, wifi);
    }
}
