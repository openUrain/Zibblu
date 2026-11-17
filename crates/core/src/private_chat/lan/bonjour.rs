use super::{DiscoveryStatus, Records, SERVICE, service_instance, valid_tag};
use std::{
    ffi::{CString, c_char, c_void},
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    os::fd::AsRawFd,
    sync::atomic::{AtomicU64, Ordering},
};

unsafe extern "C" {
    fn zibblu_wifi_state() -> u64;
    fn zibblu_bonjour_start(id: u64, name: *const c_char, tag: *const c_char, port: u16, wifi: u64);
    fn zibblu_bonjour_stop(id: u64);
    fn zibblu_bonjour_status(id: u64) -> i32;
    fn zibblu_bonjour_snapshot(id: u64, buffer: *mut u8, capacity: usize) -> usize;
}

pub(super) fn wifi() -> u64 {
    unsafe { zibblu_wifi_state() }
}

pub(super) fn bind_to_wifi(socket: &UdpSocket, index: u32) -> io::Result<()> {
    // Bind by interface as well as filtering addresses so a route change cannot select cellular.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_BOUND_IF,
            (&index as *const u32).cast::<c_void>(),
            size_of::<u32>() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) struct Bonjour(u64);

impl Bonjour {
    pub fn new(name: &str, tag: &str, port: u16, wifi: u64) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(name).expect("ASCII device ID");
        let tag = CString::new(tag).expect("ASCII account tag");
        unsafe { zibblu_bonjour_start(id, name.as_ptr(), tag.as_ptr(), port, wifi) };
        Self(id)
    }

    pub fn status(&self) -> DiscoveryStatus {
        match unsafe { zibblu_bonjour_status(self.0) } {
            0 => DiscoveryStatus::Starting,
            1 => DiscoveryStatus::Ready,
            2 => DiscoveryStatus::PermissionDenied,
            _ => DiscoveryStatus::DiscoveryFailed,
        }
    }

    pub fn refresh(&self, records: &Records) {
        let mut records = records.lock().expect("LAN discovery records");
        records.clear();
        if !matches!(self.status(), DiscoveryStatus::Ready) {
            return;
        }
        let mut buffer = vec![0; 128 * 1024];
        let size = unsafe { zibblu_bonjour_snapshot(self.0, buffer.as_mut_ptr(), buffer.len()) };
        let Some(json) = buffer.get(..size) else {
            return;
        };
        let Ok(services) =
            serde_json::from_slice::<Vec<(String, String, u16, Vec<Ipv4Addr>)>>(json)
        else {
            return;
        };
        for (name, tag, port, addresses) in services.into_iter().take(512) {
            let full_name = format!("{name}.{SERVICE}");
            if service_instance(&full_name, SERVICE).is_some() && valid_tag(&tag) {
                let addresses = addresses
                    .into_iter()
                    .map(|ip| SocketAddr::V4(SocketAddrV4::new(ip, port)))
                    .collect();
                records.insert(full_name, (tag, addresses));
            }
        }
    }

    pub fn stop(&self) {
        unsafe { zibblu_bonjour_stop(self.0) };
    }
}

impl Drop for Bonjour {
    fn drop(&mut self) {
        self.stop();
    }
}
