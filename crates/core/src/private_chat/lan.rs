use super::END;
use anyhow::{Context, Result, bail, ensure};
use if_addrs::{IfAddr, Ifv4Addr};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo, TxtProperty};
use quinn::{
    AsyncUdpSocket, ClientConfig, Connection, Endpoint, Runtime, ServerConfig, TokioRuntime,
    TransportConfig, UdpPoller,
    rustls::{RootCertStore, pki_types::PrivatePkcs8KeyDer},
    udp::{RecvMeta, Transmit},
};
use std::{
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll, ready},
    time::Duration,
};
use tokio::time::timeout;
use uuid::Uuid;

const SERVICE: &str = "_zibblu-private._udp.local.";
const TLS_NAME: &str = "zibblu-private.invalid";
const TXT_CHUNK: usize = 220;
const ATTEMPT: Duration = Duration::from_secs(3);

pub(super) struct Link {
    pub(super) connection: Connection,
    _endpoint: Endpoint,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.connection.close(END.into(), b"");
    }
}

pub(super) async fn connect(host: bool, tag: &[u8; 32], connecting: impl Fn()) -> Result<Link> {
    let networks: Vec<_> = if_addrs::get_if_addrs()
        .context("Could not read the local network interfaces.")?
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_p2p())
        .filter_map(|interface| match interface.addr {
            IfAddr::V4(address) if (1..=31).contains(&address.prefixlen) && lan_ip(address.ip) => {
                Some(address)
            }
            _ => None,
        })
        .collect();
    ensure!(
        !networks.is_empty(),
        "No private IPv4 LAN interface is available."
    );
    let networks = Arc::<[Ifv4Addr]>::from(networks);
    let discovery = Discovery::new(&networks).context("Could not start local discovery.")?;
    let result = if host {
        listen(discovery, networks, tag, connecting).await
    } else {
        browse(discovery, networks, tag, connecting).await
    };
    result.context("Could not establish a private connection on this LAN.")
}

struct Discovery {
    daemon: ServiceDaemon,
    service: Option<String>,
}

impl Discovery {
    fn new(networks: &[Ifv4Addr]) -> Result<Self> {
        let discovery = Self {
            daemon: ServiceDaemon::new()?,
            service: None,
        };
        discovery.daemon.disable_interface(IfKind::All)?;
        discovery.daemon.enable_interface(
            networks
                .iter()
                .map(|address| IpAddr::V4(address.ip))
                .collect::<Vec<_>>(),
        )?;
        Ok(discovery)
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(service) = &self.service {
            let _ = self.daemon.unregister(service);
        }
        let _ = self.daemon.shutdown();
    }
}

async fn listen(
    mut discovery: Discovery,
    networks: Arc<[Ifv4Addr]>,
    tag: &[u8; 32],
    connecting: impl Fn(),
) -> Result<Link> {
    let identity = rcgen::generate_simple_self_signed(vec![TLS_NAME.into()])?;
    let certificate = identity.cert.der();
    let mut server = ServerConfig::with_single_cert(
        vec![certificate.clone()],
        PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der()).into(),
    )?;
    server.migration(false);
    server.max_incoming(4);
    server.transport_config(transport());
    let endpoint = endpoint(networks.clone(), Some(server))?;

    let certificate = certificate.as_ref();
    ensure!(
        certificate.len() <= 2 * TXT_CHUNK,
        "The local certificate is too large for discovery."
    );
    let (first, second) = certificate.split_at(certificate.len().min(TXT_CHUNK));
    let properties = vec![
        TxtProperty::from(("pair", tag.as_slice())),
        TxtProperty::from(("cert-a", first)),
        TxtProperty::from(("cert-b", second)),
    ];
    let name = Uuid::new_v4().simple().to_string();
    let ips = networks
        .iter()
        .map(|address| IpAddr::V4(address.ip))
        .collect::<Vec<_>>();
    let info = ServiceInfo::new(
        SERVICE,
        &name,
        &format!("{name}.local."),
        ips.as_slice(),
        endpoint.local_addr()?.port(),
        properties,
    )?;
    discovery.service = Some(info.get_fullname().to_owned());
    discovery.daemon.register(info)?;
    drop(identity);

    while let Some(incoming) = endpoint.accept().await {
        connecting();
        if let Ok(Ok(connection)) = timeout(ATTEMPT, incoming).await {
            endpoint.set_server_config(None);
            return Ok(Link {
                connection,
                _endpoint: endpoint,
            });
        }
    }
    bail!("The local listener closed.")
}

async fn browse(
    discovery: Discovery,
    networks: Arc<[Ifv4Addr]>,
    tag: &[u8; 32],
    connecting: impl Fn(),
) -> Result<Link> {
    let events = discovery.daemon.browse(SERVICE)?;
    let endpoint = endpoint(networks.clone(), None)?;
    while let Ok(event) = events.recv_async().await {
        let ServiceEvent::ServiceResolved(info) = event else {
            continue;
        };
        if info.get_property_val("pair").flatten() != Some(tag.as_slice()) {
            continue;
        }
        let Some(first) = info.get_property_val("cert-a").flatten() else {
            continue;
        };
        let Some(second) = info.get_property_val("cert-b").flatten() else {
            continue;
        };
        if first.len() > TXT_CHUNK || second.len() > TXT_CHUNK {
            continue;
        }
        let mut roots = RootCertStore::empty();
        if roots.add([first, second].concat().into()).is_err() {
            continue;
        }
        let mut client = ClientConfig::with_root_certificates(Arc::new(roots))?;
        client.transport_config(transport());
        for ip in info.get_addresses_v4() {
            if !on_link(&networks, IpAddr::V4(ip)) || info.get_port() == 0 {
                continue;
            }
            connecting();
            let peer = SocketAddr::from((ip, info.get_port()));
            let attempt = endpoint.connect_with(client.clone(), peer, TLS_NAME)?;
            if let Ok(Ok(connection)) = timeout(ATTEMPT, attempt).await {
                return Ok(Link {
                    connection,
                    _endpoint: endpoint,
                });
            }
        }
    }
    bail!("Local discovery stopped.")
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

fn lan_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

fn on_link(networks: &[Ifv4Addr], ip: IpAddr) -> bool {
    let IpAddr::V4(ip) = ip else { return false };
    lan_ip(ip)
        && networks.iter().any(|network| {
            let mask = u32::from(network.netmask);
            let host = u32::from(ip);
            host & mask == u32::from(network.ip) & mask
                && (network.prefixlen >= 31 || (host & !mask != 0 && host & !mask != !mask))
        })
}

fn endpoint(networks: Arc<[Ifv4Addr]>, server: Option<ServerConfig>) -> Result<Endpoint> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    // Apply the hop limit to every QUIC packet, including path validation.
    socket.set_ttl(1)?;
    let runtime = Arc::new(TokioRuntime);
    let socket = LanSocket {
        udp: runtime.wrap_udp_socket(socket)?,
        networks,
    };
    Ok(Endpoint::new_with_abstract_socket(
        Default::default(),
        server,
        Arc::new(socket),
        runtime,
    )?)
}

#[derive(Debug)]
struct LanSocket {
    udp: Arc<dyn AsyncUdpSocket>,
    networks: Arc<[Ifv4Addr]>,
}

impl AsyncUdpSocket for LanSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.udp.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if !on_link(&self.networks, transmit.destination.ip()) {
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
        if count != 0 && !on_link(&self.networks, meta[0].addr.ip()) {
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
    fn lan_policy_rejects_public_and_off_subnet_addresses() {
        let local = Ifv4Addr {
            ip: Ipv4Addr::new(192, 168, 20, 2),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            prefixlen: 24,
            broadcast: Some(Ipv4Addr::new(192, 168, 20, 255)),
        };
        assert!(on_link(
            std::slice::from_ref(&local),
            "192.168.20.3".parse().unwrap()
        ));
        for address in [
            "192.168.20.0",
            "192.168.20.255",
            "192.168.21.3",
            "127.0.0.1",
            "8.8.8.8",
            "::1",
        ] {
            assert!(!on_link(
                std::slice::from_ref(&local),
                address.parse().unwrap()
            ));
        }
    }
}
