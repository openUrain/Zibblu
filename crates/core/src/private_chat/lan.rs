use super::identity::Identity;
use crate::lan_peer_name;
use anyhow::{Context, Result, ensure};
use hickory_resolver::{
    TokioResolver,
    proto::rr::{RData, RecordType},
};
use if_addrs::{IfAddr, Ifv4Addr};
use ipnet::Ipv4Net;
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, Runtime, TokioRuntime, TransportConfig, UdpPoller,
    udp::{RecvMeta, Transmit},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, ready},
    time::Duration,
};
use tokio::{task::JoinHandle, time::timeout};
use uuid::Uuid;

const SERVICE: &str = "_zibblu-private._udp.local.";
const ATTEMPT: Duration = Duration::from_secs(3);
type Records = Arc<Mutex<HashMap<String, (String, Vec<SocketAddr>)>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scope {
    pub interfaces: Vec<Ifv4Addr>,
    enterprise: Vec<Ipv4Net>,
    dns_domain: Option<String>,
    port: u16,
}

impl Scope {
    pub fn current() -> Result<Self> {
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
            .filter_map(|interface| match interface.addr {
                IfAddr::V4(ip) if (1..=31).contains(&ip.prefixlen) => Some(ip),
                _ => None,
            })
            .filter(|ip| {
                if enterprise.is_empty() {
                    ip.ip.is_private() || ip.ip.is_link_local()
                } else {
                    enterprise.iter().any(|net| net.contains(&ip.ip))
                }
            })
            .collect();
        interfaces.sort_by_key(|interface| interface.ip);
        ensure!(
            !interfaces.is_empty(),
            "No approved IPv4 LAN interface is available."
        );
        Ok(Self {
            interfaces,
            enterprise,
            dns_domain,
            port,
        })
    }

    fn allows(&self, peer: SocketAddr) -> bool {
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
    daemon: ServiceDaemon,
    service_name: String,
    task: JoinHandle<()>,
    resolver: Option<TokioResolver>,
}

impl Lan {
    pub fn new(scope: Scope, identity: &Identity, origin: String, account: Uuid) -> Result<Self> {
        let bind = if scope.enterprise.is_empty() {
            Ipv4Addr::UNSPECIFIED
        } else {
            scope.interfaces[0].ip
        };
        let socket = UdpSocket::bind((bind, scope.port))?;
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
            .filter(|iface| bind.is_unspecified() || iface.ip == bind)
            .map(|iface| IpAddr::V4(iface.ip))
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
        let daemon = ServiceDaemon::new()?;
        let device = Uuid::new_v4().simple().to_string();
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
        let records = Records::default();
        let discovered = records.clone();
        let task = tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let Some(tag) = info
                            .get_property_val_str("account")
                            .filter(|tag| tag.len() == 64)
                        else {
                            continue;
                        };
                        let addresses = info
                            .get_addresses_v4()
                            .into_iter()
                            .map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, info.get_port())))
                            .collect();
                        let mut discovered = discovered.lock().expect("LAN discovery records");
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
        Ok(Self {
            endpoint,
            scope,
            origin,
            bound,
            records,
            daemon,
            service_name,
            task,
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

    pub async fn candidates(&self, accounts: &[Uuid]) -> Vec<(Uuid, SocketAddr)> {
        let tags: HashMap<_, _> = accounts
            .iter()
            .map(|&account| (account_tag(&self.origin, account), account))
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
            if let Ok(Ok(entries)) =
                timeout(ATTEMPT, site_candidates(resolver, domain, &tags)).await
            {
                peers.extend(entries);
            }
        }
        peers
            .into_iter()
            .filter(|(_, addr)| self.scope.allows(*addr) && !self.bound.contains(addr))
            .collect()
    }

    pub async fn connect(
        &self,
        identity: &Identity,
        account: Uuid,
        address: SocketAddr,
    ) -> Result<Connection> {
        ensure!(
            self.scope.allows(address),
            "This device is outside the allowed LAN."
        );
        let mut client = identity.client.clone();
        client.transport_config(transport());
        let connection = self
            .endpoint
            .connect_with(client, address, &lan_peer_name(account))?;
        Ok(timeout(ATTEMPT, connection).await??)
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.endpoint.close(0_u32.into(), b"");
        self.task.abort();
        let _ = self.daemon.unregister(&self.service_name);
        let _ = self.daemon.shutdown();
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

async fn site_candidates(
    resolver: &TokioResolver,
    domain: &str,
    tags: &HashMap<String, Uuid>,
) -> Result<Vec<(Uuid, SocketAddr)>> {
    let service = format!("_zibblu-private._udp.{domain}.");
    let records = resolver.lookup(service, RecordType::PTR).await?;
    let mut addresses = Vec::new();
    for record in records.iter().take(128) {
        let RData::PTR(ptr) = record else { continue };
        let Ok(txt) = resolver.txt_lookup(ptr.0.clone()).await else {
            continue;
        };
        let account = txt
            .iter()
            .flat_map(|txt| txt.txt_data())
            .filter_map(|field| std::str::from_utf8(field).ok()?.strip_prefix("account="))
            .find_map(|tag| tags.get(tag).copied());
        let Some(account) = account else { continue };
        let Ok(services) = resolver.srv_lookup(ptr.0.clone()).await else {
            continue;
        };
        for service in services.iter() {
            let Ok(ips) = resolver.ipv4_lookup(service.target().clone()).await else {
                continue;
            };
            addresses.extend(
                ips.iter()
                    .map(|ip| (account, SocketAddr::from((ip.0, service.port())))),
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

    #[test]
    fn ordinary_and_company_networks_fail_closed() {
        let mut scope = Scope {
            interfaces: vec![Ifv4Addr {
                ip: "192.168.20.2".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
                prefixlen: 24,
                broadcast: None,
            }],
            enterprise: Vec::new(),
            dns_domain: None,
            port: 0,
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
}
