//! Cross-platform Apple TV discovery.
//!
//! Three layers run in this order and nothing about them is exposed in the UI:
//! 1. a long-lived mDNS browser that joins the multicast group on every interface
//!    and caches resolved services between scans;
//! 2. per-interface multicast queries through `ratv` for networks where the
//!    browser socket is blocked;
//! 3. a unicast sweep of the local /24, which still works when multicast or
//!    inbound multicast responses are filtered by a firewall or access point.
use if_addrs::{IfAddr, Interface};
use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent};
use ratv::{
    Endpoints,
    discovery::{self, Device, ScanOptions},
};
use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::task::JoinSet;

const COMPANION: &str = "_companion-link._tcp.local.";
const AIRPLAY: &str = "_airplay._tcp.local.";
/// Leave time for the last replies to arrive after the first device answers.
const SETTLE: Duration = Duration::from_millis(250);
#[derive(Clone, Debug)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub address: IpAddr,
    pub companion: Option<SocketAddr>,
    pub airplay: Option<SocketAddr>,
}
pub struct Browser {
    daemon: ServiceDaemon,
    companion: Receiver<ServiceEvent>,
    airplay: Receiver<ServiceEvent>,
    services: BTreeMap<String, Box<ResolvedService>>,
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}
impl Browser {
    pub fn new() -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let browse =
            || Ok::<_, mdns_sd::Error>((daemon.browse(COMPANION)?, daemon.browse(AIRPLAY)?));
        let (companion, airplay) = match browse() {
            Ok(receivers) => receivers,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error.to_string());
            }
        };
        Ok(Self {
            daemon,
            companion,
            airplay,
            services: BTreeMap::new(),
        })
    }
    fn absorb(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::ServiceResolved(service) => {
                self.services.insert(service.fullname.clone(), service);
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                self.services.remove(&name);
            }
            _ => {}
        }
    }
    pub fn devices(&self, own: &[IpAddr]) -> Vec<Found> {
        let mut hosts: BTreeMap<&str, Vec<&ResolvedService>> = BTreeMap::new();
        for service in self.services.values() {
            hosts
                .entry(service.get_hostname())
                .or_default()
                .push(service);
        }
        hosts
            .into_values()
            .filter_map(|services| from_services(&services, own))
            .collect()
    }
    /// Cached results come back immediately; otherwise wait for fresh events.
    pub async fn scan(
        &mut self,
        wanted: Option<&str>,
        own: &[IpAddr],
        wait: Duration,
    ) -> Result<Vec<Found>, String> {
        // With an empty cache, keep listening for the whole window so every
        // receiver on the network is reported together. A warm cache only
        // needs a short settle window for late arrivals.
        let warm = !self.devices(own).is_empty();
        let deadline = tokio::time::Instant::now() + wait;
        let mut settle: Option<tokio::time::Instant> = None;
        loop {
            let now = tokio::time::Instant::now();
            let limit = settle.unwrap_or(deadline);
            if now >= limit {
                break;
            }
            if settle.is_none() && self.ready(wanted, warm, own) {
                settle = Some(now + SETTLE);
                continue;
            }
            let event = tokio::select! {
                event=self.companion.recv_async()=>event,
                event=self.airplay.recv_async()=>event,
                _=tokio::time::sleep_until(limit)=>break,
            }
            .map_err(|_| "Network discovery stopped. Please restart the app.".to_string())?;
            self.absorb(event);
        }
        Ok(self.devices(own))
    }
    fn ready(&self, wanted: Option<&str>, warm: bool, own: &[IpAddr]) -> bool {
        match wanted {
            Some(id) => self
                .devices(own)
                .iter()
                .any(|d| d.id == id && d.companion.is_some()),
            None => warm,
        }
    }
}
fn from_services(services: &[&ResolvedService], own: &[IpAddr]) -> Option<Found> {
    // Both services must resolve on one host: Companion for control, AirPlay
    // for a stable device id and model. Any receiver that presents itself as
    // an Apple TV (including AirPlay receivers like UxPlay) is kept.
    let companion = services.iter().find(|s| s.ty_domain == COMPANION)?;
    let airplay = services.iter().find(|s| s.ty_domain == AIRPLAY)?;
    if !airplay
        .get_property_val_str("model")
        .is_some_and(|model| model.starts_with("AppleTV"))
    {
        return None;
    }
    let mut addresses = services
        .iter()
        .flat_map(|s| s.get_addresses_v4())
        .map(IpAddr::V4)
        .filter(|ip| !ip.is_loopback() && !own.contains(ip))
        .collect::<Vec<_>>();
    addresses.sort();
    addresses.dedup();
    let address = *addresses.first()?;
    let id = airplay
        .get_property_val_str("deviceid")
        .or_else(|| companion.get_property_val_str("rpMRtID"))
        .unwrap_or(companion.get_hostname())
        .to_owned();
    let name = companion
        .fullname
        .strip_suffix(&format!(".{}", companion.ty_domain))
        .unwrap_or(&companion.fullname)
        .to_owned();
    Some(Found {
        id,
        name,
        address,
        companion: Some(SocketAddr::new(address, companion.port)),
        airplay: Some(SocketAddr::new(address, airplay.port)),
    })
}
fn is_appletv(device: &Device) -> bool {
    if !device
        .model()
        .is_some_and(|model| model.starts_with("AppleTV"))
    {
        return false;
    }
    let endpoints = Endpoints::from(device);
    endpoints.companion.is_some() && endpoints.airplay.is_some()
}
fn from_device(device: &Device, own: &[IpAddr]) -> Option<Found> {
    let address = device
        .addresses
        .iter()
        .copied()
        .find(|ip| !ip.is_loopback() && !own.contains(ip))?;
    if !is_appletv(device) {
        return None;
    }
    let endpoints = Endpoints::from(device);
    Some(Found {
        id: device.identifier().unwrap_or(&device.host).into(),
        name: device.name().into(),
        address,
        companion: endpoints
            .companion
            .map(|s| SocketAddr::new(address, s.port())),
        airplay: endpoints
            .airplay
            .map(|s| SocketAddr::new(address, s.port())),
    })
}
/// Real LAN interfaces, skipping loopback, link-local and VPN/tunnel adapters.
fn lan_ipv4(interface: &Interface) -> Option<Ipv4Addr> {
    let IfAddr::V4(v4) = &interface.addr else {
        return None;
    };
    usable_address(v4.ip, v4.prefixlen, interface.is_p2p()).then_some(v4.ip)
}
fn usable_address(ip: Ipv4Addr, prefixlen: u8, p2p: bool) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !p2p && prefixlen < 31
}
fn interface_ips(interfaces: &[Interface]) -> Vec<Ipv4Addr> {
    let mut ips = interfaces.iter().filter_map(lan_ipv4).collect::<Vec<_>>();
    ips.sort();
    ips.dedup();
    ips.truncate(16);
    ips
}
pub fn own_addresses(interfaces: &[Interface]) -> Vec<IpAddr> {
    interfaces.iter().map(|interface| interface.ip()).collect()
}
async fn collect(mut tasks: JoinSet<Vec<Device>>, own: &[IpAddr]) -> Vec<Found> {
    let mut found = Vec::new();
    while let Some(Ok(devices)) = tasks.join_next().await {
        found.extend(devices.iter().filter_map(|device| from_device(device, own)));
    }
    found
}
/// Asks every interface separately, so a VPN or virtual adapter cannot swallow
/// the query and the replies arrive on the same adapter that sent them.
pub async fn scan_interfaces(
    interfaces: &[Interface],
    own: &[IpAddr],
    wait: Duration,
) -> Vec<Found> {
    let mut tasks = JoinSet::new();
    let ips = interface_ips(interfaces);
    if ips.is_empty() {
        let mut options = ScanOptions::default();
        options.timeout = wait;
        tasks.spawn(async move {
            discovery::scan_until(&options, is_appletv)
                .await
                .unwrap_or_default()
        });
    }
    for ip in ips {
        let mut options = ScanOptions::default();
        options.timeout = wait;
        options.bind = IpAddr::V4(ip);
        tasks.spawn(async move {
            discovery::scan_until(&options, is_appletv)
                .await
                .unwrap_or_default()
        });
    }
    collect(tasks, own).await
}
/// Queries explicit mDNS targets, for example a saved address.
pub async fn scan_direct(options: &ScanOptions, own: &[IpAddr]) -> Vec<Found> {
    scan_devices(options.clone())
        .await
        .iter()
        .filter_map(|device| from_device(device, own))
        .collect()
}
/// Unicast mDNS queries for every host in the local /24 of each interface.
/// Firewalls allow replies to our own outbound packets, so this reaches an
/// Apple TV even when multicast is dropped entirely.
pub fn sweep_targets(interfaces: &[Interface]) -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    for interface in interfaces {
        let Some(ip) = lan_ipv4(interface) else {
            continue;
        };
        let IfAddr::V4(v4) = &interface.addr else {
            continue;
        };
        let prefix = if v4.prefixlen == 0 {
            u32::from(v4.netmask).count_ones()
        } else {
            u32::from(v4.prefixlen)
        };
        targets.extend(subnet_targets(ip, prefix));
    }
    targets.sort();
    targets.dedup();
    targets
}
/// Hosts of the /24 that contains `ip`, even on wider subnets, excluding the
/// machine itself.
fn subnet_targets(ip: Ipv4Addr, prefix: u32) -> Vec<SocketAddr> {
    let prefix = prefix.max(24);
    let host = u32::from(ip);
    let network = host & (u32::MAX << (32 - prefix));
    let size = 1u32 << (32 - prefix);
    ((network + 1)..(network + size - 1))
        .filter(|address| *address != host)
        .map(|address| SocketAddr::new(IpAddr::V4(Ipv4Addr::from(address)), discovery::MDNS_PORT))
        .collect()
}
/// Number of sockets used for the unicast sweep. Some Wi-Fi drivers serialize
/// sends on a single socket (measured: 3 s for 253 packets), so the targets are
/// split across sockets, which brings the same burst below a quarter second.
const SWEEP_SOCKETS: usize = 16;
async fn scan_devices(mut options: ScanOptions) -> Vec<Device> {
    options.timeout = options.timeout.max(Duration::from_millis(200));
    discovery::scan_until(&options, is_appletv)
        .await
        .unwrap_or_default()
}
pub async fn sweep_interfaces(
    interfaces: &[Interface],
    own: &[IpAddr],
    wait: Duration,
) -> Vec<Found> {
    let targets = sweep_targets(interfaces);
    if targets.is_empty() {
        return Vec::new();
    }
    let size = targets.len().div_ceil(SWEEP_SOCKETS).max(1);
    let mut tasks = JoinSet::new();
    let mut targets = targets.into_iter();
    loop {
        let batch = targets.by_ref().take(size).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }
        let mut options = ScanOptions::default();
        options.timeout = wait;
        options.targets = batch;
        tasks.spawn(scan_devices(options));
    }
    collect(tasks, own).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use mdns_sd::ServiceInfo;
    fn service(kind: &str, model: &str, port: u16) -> ResolvedService {
        let properties = if kind == AIRPLAY {
            vec![
                ("deviceid", "AA:BB:CC:DD:EE:FF"),
                ("model", model),
                ("osvers", "26.5"),
            ]
        } else {
            vec![("rpMRtID", "companion-id"), ("rpMd", model)]
        };
        ServiceInfo::new(
            kind,
            "Living Room",
            "living-room.local.",
            "192.0.2.11",
            port,
            &properties[..],
        )
        .unwrap()
        .as_resolved_service()
    }
    #[test]
    fn combines_bonjour_services_and_keeps_stable_identifier() {
        let companion = service(COMPANION, "AppleTV11,1", 49153);
        let airplay = service(AIRPLAY, "AppleTV11,1", 7000);
        let found = from_services(&[&companion, &airplay], &[]).unwrap();
        assert_eq!(found.id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(found.name, "Living Room");
        assert_eq!(found.companion.unwrap().port(), 49153);
        assert_eq!(found.airplay.unwrap().port(), 7000);
    }
    #[test]
    fn excludes_this_machine_and_non_tv_airplay_receivers() {
        let companion = service(COMPANION, "AppleTV11,1", 49153);
        let tv = service(AIRPLAY, "AppleTV11,1", 7000);
        assert!(from_services(&[&companion, &tv], &["192.0.2.11".parse().unwrap()]).is_none());
        let computer = service(AIRPLAY, "Mac16,7", 7000);
        assert!(from_services(&[&companion, &computer], &[]).is_none());
        assert!(
            from_services(&[&tv], &[]).is_none(),
            "companion is required"
        );
    }
    #[test]
    fn keeps_tvs_that_emulate_the_companion_protocol() {
        // UxPlay-style receivers on a smart TV answer as model=AppleTV14,1 and
        // are legitimate targets, so they must stay in the list.
        let airplay = ServiceInfo::new(
            AIRPLAY,
            "Smart TV Remote",
            "smart-tv.local.",
            "192.0.2.12",
            60498,
            &[
                ("deviceid", "02:00:00:00:00:02"),
                ("model", "AppleTV14,1"),
                ("vv", "2"),
            ][..],
        )
        .unwrap()
        .as_resolved_service();
        let companion = ServiceInfo::new(
            COMPANION,
            "Smart TV Remote",
            "smart-tv.local.",
            "192.0.2.12",
            49153,
            &[("rpMd", "AppleTV14,1")][..],
        )
        .unwrap()
        .as_resolved_service();
        let found = from_services(&[&companion, &airplay], &[]).unwrap();
        assert_eq!(found.id, "02:00:00:00:00:02");
        assert_eq!(found.name, "Smart TV Remote");
    }
    #[test]
    fn sweeps_the_local_subnet_without_this_machine() {
        let mut targets = subnet_targets("192.168.1.40".parse().unwrap(), 24);
        targets.extend(subnet_targets("10.0.0.9".parse().unwrap(), 16));
        assert_eq!(targets.len(), 506);
        assert!(targets.contains(&"192.168.1.11:5353".parse().unwrap()));
        assert!(!targets.contains(&"192.168.1.40:5353".parse().unwrap()));
        assert!(targets.contains(&"10.0.0.200:5353".parse().unwrap()));
        assert!(!targets.iter().any(|t| t.ip().is_loopback()));
    }
    #[test]
    fn ignores_point_to_point_and_link_local_addresses() {
        assert!(!usable_address("169.254.1.2".parse().unwrap(), 16, false));
        assert!(!usable_address("10.1.1.1".parse().unwrap(), 31, false));
        assert!(!usable_address("10.1.1.1".parse().unwrap(), 24, true));
        assert!(!usable_address("127.0.0.1".parse().unwrap(), 8, false));
        assert!(usable_address("192.168.1.40".parse().unwrap(), 24, false));
    }
    #[tokio::test]
    #[ignore = "requires an Apple TV on the local network"]
    async fn finds_real_appletv_on_the_lan() {
        let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
        let own = own_addresses(&interfaces);
        let mut browser = Browser::new().unwrap();
        let cached = browser
            .scan(None, &own, Duration::from_secs(3))
            .await
            .unwrap();
        let multicast = scan_interfaces(&interfaces, &own, Duration::from_secs(3)).await;
        let sweep = sweep_interfaces(&interfaces, &own, Duration::from_secs(3)).await;
        // Print names only: pairing ids are real hardware addresses and this
        // repository is public.
        for (layer, devices) in [
            ("browser", &cached),
            ("multicast", &multicast),
            ("sweep", &sweep),
        ] {
            for device in devices.iter() {
                println!(
                    "{layer}: {} companion={} airplay={}",
                    device.name,
                    device.companion.is_some(),
                    device.airplay.is_some()
                );
            }
        }
        assert!(
            !cached.is_empty() || !multicast.is_empty(),
            "both mDNS paths failed (interfaces: {own:?})"
        );
        assert!(
            cached
                .iter()
                .chain(&multicast)
                .chain(&sweep)
                .any(|d| d.companion.is_some()),
            "discovered Apple TV without a companion endpoint"
        );
    }
}
