//! Find cameras on the network: ONVIF WS-Discovery (the standard way IP
//! cameras announce themselves) plus a quick check of the local network
//! for anything answering on the RTSP video port (554).
//!
//! A scan also runs once a day; cameras not seen before are reported.

use crate::manager::Manager;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Found {
    pub ip: String,
    /// ONVIF service port (0 = no ONVIF answer).
    pub onvif_port: u16,
    /// Name and model the camera announced (from its ONVIF scopes).
    pub name: String,
    pub hardware: String,
    pub manufacturer: String,
    /// Port 554 answered.
    pub rtsp: bool,
    /// Already added as a camera.
    pub added: bool,
}

fn probe_message() -> String {
    let id = format!("uuid:{:032x}", rand_id());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><e:Header><w:MessageID>{id}</w:MessageID><w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action e:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header><e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>"#
    )
}

fn rand_id() -> u128 {
    use rand_core::RngCore;
    let mut b = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut b);
    u128::from_le_bytes(b)
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read one ProbeMatch reply.
pub fn parse_match(xml: &str) -> Option<Found> {
    let xaddrs = crate::onvif::tag_text(xml, "XAddrs")?;
    let first = xaddrs
        .split_whitespace()
        .find(|u| u.starts_with("http://"))?;
    let hostport = first.trim_start_matches("http://").split('/').next()?;
    let (ip, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(80)),
        None => (hostport.to_string(), 80),
    };
    ip.parse::<Ipv4Addr>().ok()?;
    let scopes = crate::onvif::tag_text(xml, "Scopes").unwrap_or_default();
    let scope = |key: &str| {
        scopes
            .split_whitespace()
            .find_map(|s| s.strip_prefix(&format!("onvif://www.onvif.org/{key}/")))
            .map(pct_decode)
            .unwrap_or_default()
    };
    Some(Found {
        ip,
        onvif_port: port,
        name: scope("name"),
        hardware: scope("hardware"),
        manufacturer: scope("mfr"),
        ..Default::default()
    })
}

async fn ws_discovery(wait: Duration) -> Vec<Found> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else {
        return vec![];
    };
    let _ = sock.set_broadcast(true);
    let msg = probe_message();
    let target: SocketAddr = "239.255.255.250:3702".parse().unwrap();
    for _ in 0..2 {
        let _ = sock.send_to(msg.as_bytes(), target).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let mut out: BTreeMap<String, Found> = BTreeMap::new();
    let mut buf = vec![0u8; 65536];
    let end = tokio::time::Instant::now() + wait;
    while let Ok(Ok((n, _))) = tokio::time::timeout_at(end, sock.recv_from(&mut buf)).await {
        if let Some(f) = parse_match(&String::from_utf8_lossy(&buf[..n])) {
            out.entry(f.ip.clone()).or_insert(f);
        }
    }
    out.into_values().collect()
}

/// Hosts on this computer's local /24 networks answering on port 554.
async fn rtsp_sweep() -> Vec<String> {
    let mut nets = HashSet::new();
    for a in crate::web::local_addresses() {
        if let Ok(ip) = a.parse::<Ipv4Addr>()
            && ip.is_private()
        {
            let o = ip.octets();
            nets.insert([o[0], o[1], o[2]]);
        }
    }
    sweep(nets.into_iter().collect(), 554).await
}

async fn sweep(nets: Vec<[u8; 3]>, port: u16) -> Vec<String> {
    let sem = Arc::new(tokio::sync::Semaphore::new(128));
    let mut tasks = Vec::new();
    for n in nets {
        for h in 1..=254u8 {
            let ip = Ipv4Addr::new(n[0], n[1], n[2], h);
            let sem = sem.clone();
            tasks.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok()?;
                let ok = tokio::time::timeout(
                    Duration::from_millis(400),
                    tokio::net::TcpStream::connect((ip, port)),
                )
                .await;
                matches!(ok, Ok(Ok(_))).then(|| ip.to_string())
            }));
        }
    }
    let mut out = Vec::new();
    for t in tasks {
        if let Ok(Some(ip)) = t.await {
            out.push(ip);
        }
    }
    out
}

/// Scan the network now.
pub async fn scan(mgr: &Manager) -> Vec<Found> {
    let job = crate::jobs::start("scan", "Looking for cameras on the network");
    job.step("ONVIF discovery and port 554 on local networks");
    let (onvif, rtsp) = tokio::join!(ws_discovery(Duration::from_secs(3)), rtsp_sweep());
    let mut all: BTreeMap<String, Found> = onvif.into_iter().map(|f| (f.ip.clone(), f)).collect();
    for ip in rtsp {
        all.entry(ip.clone())
            .or_insert(Found {
                ip,
                ..Default::default()
            })
            .rtsp = true;
    }
    // Which are already cameras here (by the address in their stream URLs).
    let cfg = mgr.config();
    let known: HashSet<String> = cfg
        .cameras
        .iter()
        .flat_map(|c| [c.url.clone(), c.live_url.clone(), c.ptz.host.clone()])
        .filter_map(|u| {
            let rest = u.split_once("://").map(|x| x.1).unwrap_or(&u).to_string();
            let host = rest
                .rsplit('@')
                .next()
                .unwrap_or(&rest)
                .split(['/', ':', '?'])
                .next()
                .unwrap_or("")
                .to_string();
            (!host.is_empty()).then_some(host)
        })
        .collect();
    let mut list: Vec<Found> = all.into_values().collect();
    for f in &mut list {
        if f.rtsp == false && f.onvif_port > 0 {
            // ONVIF cameras nearly always stream on 554 too.
            f.rtsp = true;
        }
        f.added = known.contains(&f.ip);
    }
    list.sort_by_key(|f| f.ip.parse::<Ipv4Addr>().map(u32::from).unwrap_or(0));
    list
}

/// Once a day: report cameras on the network that weren't there before.
pub async fn daily(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(300)).await;
    let file = mgr.data_dir.join("discovered.json");
    loop {
        let found = scan(&mgr).await;
        let seen: HashSet<String> = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let new: Vec<&Found> = found
            .iter()
            .filter(|f| !f.added && !seen.contains(&f.ip))
            .collect();
        if !seen.is_empty() || !found.is_empty() {
            for f in &new {
                let what = [
                    f.manufacturer.as_str(),
                    f.hardware.as_str(),
                    f.name.as_str(),
                ]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
                warn!(target: "event", "New camera found on the network: {}{}", f.ip, if what.is_empty() { String::new() } else { format!(" ({what})") });
            }
        }
        *mgr.new_cameras.lock().unwrap() = found
            .iter()
            .filter(|f| !f.added)
            .cloned()
            .filter(|f| new.iter().any(|n| n.ip == f.ip) || !seen.contains(&f.ip))
            .collect();
        let all: HashSet<String> = seen
            .into_iter()
            .chain(found.iter().map(|f| f.ip.clone()))
            .collect();
        if let Ok(b) = serde_json::to_vec(&all) {
            let _ = std::fs::write(&file, b);
        }
        info!(
            "network scan: {} camera(s) found, {} not added",
            found.len(),
            found.iter().filter(|f| !f.added).count()
        );
        tokio::time::sleep(Duration::from_secs(86_400)).await;
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn sweep_finds_open_port() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let _ = l.accept().await;
            }
        });
        let found = super::sweep(vec![[127, 0, 0]], port).await;
        assert_eq!(found, vec!["127.0.0.1".to_string()]);
    }

    #[test]
    fn reads_probe_match() {
        let xml = r#"<SOAP-ENV:Envelope><SOAP-ENV:Body><d:ProbeMatches><d:ProbeMatch><wsa:EndpointReference><wsa:Address>urn:uuid:x</wsa:Address></wsa:EndpointReference>
            <d:Types>dn:NetworkVideoTransmitter</d:Types><d:Scopes>onvif://www.onvif.org/type/video_encoder onvif://www.onvif.org/name/Front%20Door onvif://www.onvif.org/hardware/DS-2CD2387G2 onvif://www.onvif.org/mfr/HIKVISION</d:Scopes>
            <d:XAddrs>http://192.168.1.64/onvif/device_service http://[fe80::1]/onvif/device_service</d:XAddrs></d:ProbeMatch></d:ProbeMatches></SOAP-ENV:Body></SOAP-ENV:Envelope>"#;
        let f = super::parse_match(xml).unwrap();
        assert_eq!(f.ip, "192.168.1.64");
        assert_eq!(f.onvif_port, 80);
        assert_eq!(f.name, "Front Door");
        assert_eq!(f.hardware, "DS-2CD2387G2");
        assert_eq!(f.manufacturer, "HIKVISION");
    }
}
