//! Camera settings over ONVIF: device information, network (IP address,
//! DHCP, gateway, DNS, host name), date & time, video streams (resolution,
//! frame rate, bitrate) and picture (brightness, contrast, day/night).
//!
//! Cameras differ in what they allow; each part is read separately and a
//! part the camera doesn't support is reported instead of failing the rest.

use crate::onvif::{Onvif, Target, attr_values, capability_xaddr, tag_text, xml_escape};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DEVICE: &str = "http://www.onvif.org/ver10/device/wsdl";
const MEDIA1: &str = "http://www.onvif.org/ver10/media/wsdl";
const MEDIA2: &str = "http://www.onvif.org/ver20/media/wsdl";
const IMAGING: &str = "http://www.onvif.org/ver20/imaging/wsdl";

// ---------------------------------------------------------------- XML helpers

/// Every element with local name `name`: (opening tag text, inner XML).
pub fn elements<'a>(xml: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = xml[pos..].find('<') {
        let start = pos + i + 1;
        let Some(e) = xml[start..].find('>') else {
            break;
        };
        let end = start + e;
        let tag = &xml[start..end];
        let qname = tag.split_whitespace().next().unwrap_or("");
        let local = qname.rsplit(':').next().unwrap_or(qname);
        if local == name && !tag.starts_with('/') && !tag.starts_with('?') {
            if tag.ends_with('/') {
                out.push((tag, ""));
            } else if let Some(c) = xml[end + 1..].find(&format!("</{qname}>")) {
                out.push((tag, &xml[end + 1..end + 1 + c]));
                pos = end + 1 + c;
                continue;
            }
        }
        pos = end + 1;
    }
    out
}

fn text(xml: &str, name: &str) -> String {
    tag_text(xml, name).unwrap_or_default()
}

fn num(xml: &str, name: &str) -> Option<f64> {
    tag_text(xml, name).and_then(|v| v.trim().parse().ok())
}

fn attr(tag: &str, a: &str) -> Option<String> {
    let key = format!("{a}=\"");
    let i = tag.find(&key)? + key.len();
    Some(tag[i..].split('"').next()?.to_string())
}

/// Replace the text of the first element `name` inside `xml`.
fn set_text(xml: &str, name: &str, value: &str) -> String {
    let mut pos = 0;
    while let Some(i) = xml[pos..].find('<') {
        let start = pos + i + 1;
        let Some(e) = xml[start..].find('>') else {
            break;
        };
        let end = start + e;
        let tag = &xml[start..end];
        let qname = tag.split_whitespace().next().unwrap_or("");
        let local = qname.rsplit(':').next().unwrap_or(qname);
        if local == name && !tag.starts_with('/') && !tag.ends_with('/') {
            if let Some(c) = xml[end + 1..].find("</") {
                return format!(
                    "{}{}{}",
                    &xml[..end + 1],
                    xml_escape(value),
                    &xml[end + 1 + c..]
                );
            }
        }
        pos = end + 1;
    }
    xml.to_string()
}

/// Namespace declarations of a reply, so a piece of it can be sent back.
fn namespaces(xml: &str) -> String {
    let mut seen = BTreeMap::new();
    let mut rest = xml;
    while let Some(i) = rest.find("xmlns:") {
        let r = &rest[i + 6..];
        if let Some(eq) = r.find("=\"") {
            let prefix = &r[..eq];
            let v = &r[eq + 2..];
            if let Some(q) = v.find('"')
                && !prefix.contains(|c: char| c.is_whitespace() || c == '>')
            {
                seen.entry(prefix.to_string())
                    .or_insert_with(|| v[..q].to_string());
            }
        }
        rest = r;
    }
    seen.iter()
        .map(|(p, u)| format!(r#" xmlns:{p}="{u}""#))
        .collect()
}

fn fix_xaddr(t: &Target, u: &str) -> String {
    // Cameras sometimes report an internal address: keep the path, use ours.
    match u
        .split_once("://")
        .and_then(|(_, r)| r.find('/').map(|i| r[i..].to_string()))
    {
        Some(path) => format!("http://{}:{}{}", t.host, t.port, path),
        None => u.to_string(),
    }
}

// ---------------------------------------------------------------- data

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeviceInfo {
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub serial: String,
    pub hardware: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Network {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub mac: String,
    pub dhcp: bool,
    pub ip: String,
    pub prefix: u8,
    pub gateway: String,
    pub dns: Vec<String>,
    #[serde(default)]
    pub dns_dhcp: bool,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimeSettings {
    /// "NTP" or "Manual".
    pub mode: String,
    pub tz: String,
    pub ntp_dhcp: bool,
    pub ntp: Vec<String>,
    /// The camera's clock (UTC), if it reported it.
    #[serde(default)]
    pub utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct VideoStream {
    pub token: String,
    pub name: String,
    pub encoding: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bitrate: u32,
    pub gov: u32,
    pub resolutions: Vec<(u32, u32)>,
    pub fps_options: Vec<f64>,
    pub fps_min: f64,
    pub fps_max: f64,
    pub bitrate_min: u32,
    pub bitrate_max: u32,
    pub encodings: Vec<String>,
    /// "media2" (supports H.265) or "media1".
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Imaging {
    pub source: String,
    pub brightness: Option<f64>,
    pub contrast: Option<f64>,
    pub saturation: Option<f64>,
    pub sharpness: Option<f64>,
    pub ir_cut: Option<String>,
    pub ranges: BTreeMap<String, (f64, f64)>,
    pub ir_modes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeviceReport {
    pub info: Option<DeviceInfo>,
    pub network: Option<Network>,
    pub time: Option<TimeSettings>,
    pub video: Vec<VideoStream>,
    pub imaging: Option<Imaging>,
    /// What could not be read, per part.
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoChange {
    pub token: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bitrate: u32,
    #[serde(default)]
    pub gov: u32,
    #[serde(default)]
    pub encoding: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImagingChange {
    pub brightness: Option<f64>,
    pub contrast: Option<f64>,
    pub saturation: Option<f64>,
    pub sharpness: Option<f64>,
    pub ir_cut: Option<String>,
}

// ---------------------------------------------------------------- requests

struct Services {
    media1: Option<String>,
    media2: Option<String>,
    imaging: Option<String>,
}

impl Onvif {
    async fn dev(&self, t: &Target, op: &str, inner: &str) -> Result<String> {
        self.call(
            t,
            &t.device_url(),
            &format!("{DEVICE}/{op}"),
            &format!(r#"<{op} xmlns="{DEVICE}">{inner}</{op}>"#),
        )
        .await
    }

    async fn services(&self, t: &Target) -> Result<Services> {
        let caps = self
            .dev(t, "GetCapabilities", "<Category>All</Category>")
            .await?;
        let mut s = Services {
            media1: capability_xaddr(&caps, "Media").map(|u| fix_xaddr(t, &u)),
            media2: None,
            imaging: capability_xaddr(&caps, "Imaging").map(|u| fix_xaddr(t, &u)),
        };
        // Media 2 (needed for H.265 settings) is only listed by GetServices.
        if let Ok(sv) = self
            .dev(
                t,
                "GetServices",
                "<IncludeCapability>false</IncludeCapability>",
            )
            .await
        {
            for (_, inner) in elements(&sv, "Service") {
                let ns = text(inner, "Namespace");
                let x = text(inner, "XAddr");
                if ns == MEDIA2 && !x.is_empty() {
                    s.media2 = Some(fix_xaddr(t, &x));
                }
                if ns == IMAGING && s.imaging.is_none() && !x.is_empty() {
                    s.imaging = Some(fix_xaddr(t, &x));
                }
            }
        }
        Ok(s)
    }

    /// Read everything the camera lets us see.
    pub async fn device_report(&self, t: &Target) -> DeviceReport {
        let mut r = DeviceReport::default();
        let mut err = |k: &str, e: anyhow::Error| {
            r.errors.insert(k.into(), format!("{e:#}"));
        };
        match self.dev(t, "GetDeviceInformation", "").await {
            Ok(x) => {
                r.info = Some(DeviceInfo {
                    manufacturer: text(&x, "Manufacturer"),
                    model: text(&x, "Model"),
                    firmware: text(&x, "FirmwareVersion"),
                    serial: text(&x, "SerialNumber"),
                    hardware: text(&x, "HardwareId"),
                });
            }
            Err(e) => {
                // Not reachable at all: no point asking for the rest.
                err("connection", e);
                return r;
            }
        }
        match self.network(t).await {
            Ok(n) => r.network = Some(n),
            Err(e) => err("network", e),
        }
        match self.time(t).await {
            Ok(v) => r.time = Some(v),
            Err(e) => err("time", e),
        }
        match self.services(t).await {
            Ok(sv) => {
                match self.video(t, &sv).await {
                    Ok(v) => r.video = v,
                    Err(e) => err("video", e),
                }
                match self.imaging(t, &sv).await {
                    Ok(v) => r.imaging = Some(v),
                    Err(e) => err("imaging", e),
                }
            }
            Err(e) => err("video", e),
        }
        r
    }

    async fn network(&self, t: &Target) -> Result<Network> {
        let x = self.dev(t, "GetNetworkInterfaces", "").await?;
        let (tag, nic) = elements(&x, "NetworkInterfaces")
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("the camera reported no network interface"))?;
        let ipv4 = elements(nic, "IPv4")
            .into_iter()
            .next()
            .map(|e| e.1)
            .unwrap_or("");
        let dhcp = text(ipv4, "DHCP") == "true";
        let pick = |sec: &str| {
            elements(ipv4, sec).into_iter().next().map(|(_, v)| {
                (
                    text(v, "Address"),
                    num(v, "PrefixLength").unwrap_or(24.0) as u8,
                )
            })
        };
        let (ip, prefix) = if dhcp {
            pick("FromDHCP").or_else(|| pick("Manual"))
        } else {
            pick("Manual").or_else(|| pick("FromDHCP"))
        }
        .unwrap_or_default();
        let mut n = Network {
            token: attr(tag, "token").unwrap_or_default(),
            mac: text(nic, "HwAddress"),
            dhcp,
            ip,
            prefix,
            ..Default::default()
        };
        if let Ok(g) = self.dev(t, "GetNetworkDefaultGateway", "").await {
            n.gateway = text(&g, "IPv4Address");
        }
        if let Ok(d) = self.dev(t, "GetDNS", "").await {
            n.dns_dhcp = text(&d, "FromDHCP") == "true";
            let sec = if n.dns_dhcp {
                "DNSFromDHCP"
            } else {
                "DNSManual"
            };
            n.dns = elements(&d, sec)
                .into_iter()
                .map(|(_, v)| text(v, "IPv4Address"))
                .filter(|v| !v.is_empty())
                .collect();
        }
        if let Ok(h) = self.dev(t, "GetHostname", "").await {
            n.hostname = text(&h, "Name");
        }
        Ok(n)
    }

    /// Change the network settings. Returns true if the camera says it must
    /// restart for them to apply.
    pub async fn set_network(&self, t: &Target, n: &Network) -> Result<bool> {
        let cur = self.network(t).await?;
        let token = if n.token.is_empty() {
            cur.token.clone()
        } else {
            n.token.clone()
        };
        if !n.dhcp {
            let ip: std::net::Ipv4Addr =
                n.ip.trim()
                    .parse()
                    .map_err(|_| anyhow!("“{}” is not a valid IP address", n.ip))?;
            if !(1..=32).contains(&n.prefix) {
                bail!("the subnet prefix must be between 1 and 32");
            }
            if ip.is_unspecified() || ip.is_broadcast() {
                bail!("“{ip}” can't be used as a camera address");
            }
        }
        // Host name, DNS and gateway first: changing the address may cut us off.
        if !n.hostname.trim().is_empty() && n.hostname.trim() != cur.hostname {
            self.dev(
                t,
                "SetHostname",
                &format!("<Name>{}</Name>", xml_escape(n.hostname.trim())),
            )
            .await
            .map_err(|e| anyhow!("host name: {e:#}"))?;
        }
        if !n.dhcp {
            let dns: String = n
                .dns
                .iter()
                .map(|d| d.trim())
                .filter(|d| !d.is_empty())
                .map(|d| format!("<DNSManual><tt:Type>IPv4</tt:Type><tt:IPv4Address>{}</tt:IPv4Address></DNSManual>", xml_escape(d)))
                .collect();
            if !dns.is_empty() {
                self.dev(t, "SetDNS", &format!("<FromDHCP>false</FromDHCP>{dns}"))
                    .await
                    .map_err(|e| anyhow!("DNS: {e:#}"))?;
            }
            if !n.gateway.trim().is_empty() && n.gateway.trim() != cur.gateway {
                self.dev(
                    t,
                    "SetNetworkDefaultGateway",
                    &format!(
                        "<IPv4Address>{}</IPv4Address>",
                        xml_escape(n.gateway.trim())
                    ),
                )
                .await
                .map_err(|e| anyhow!("gateway: {e:#}"))?;
            }
        }
        if n.dhcp == cur.dhcp && (n.dhcp || (n.ip.trim() == cur.ip && n.prefix == cur.prefix)) {
            return Ok(false);
        }
        let ipv4 = if n.dhcp {
            "<tt:Enabled>true</tt:Enabled><tt:DHCP>true</tt:DHCP>".to_string()
        } else {
            format!(
                "<tt:Enabled>true</tt:Enabled><tt:Manual><tt:Address>{}</tt:Address><tt:PrefixLength>{}</tt:PrefixLength></tt:Manual><tt:DHCP>false</tt:DHCP>",
                xml_escape(n.ip.trim()),
                n.prefix
            )
        };
        let r = self
            .dev(
                t,
                "SetNetworkInterfaces",
                &format!(
                    "<InterfaceToken>{}</InterfaceToken><NetworkInterface><tt:Enabled>true</tt:Enabled><tt:IPv4>{ipv4}</tt:IPv4></NetworkInterface>",
                    xml_escape(&token)
                ),
            )
            .await
            .map_err(|e| anyhow!("IP address: {e:#}"))?;
        Ok(text(&r, "RebootNeeded") == "true")
    }

    async fn time(&self, t: &Target) -> Result<TimeSettings> {
        let x = self.dev(t, "GetSystemDateAndTime", "").await?;
        let mut s = TimeSettings {
            mode: text(&x, "DateTimeType"),
            tz: elements(&x, "TimeZone")
                .first()
                .map(|e| text(e.1, "TZ"))
                .unwrap_or_default(),
            ..Default::default()
        };
        if let Some((_, u)) = elements(&x, "UTCDateTime").first() {
            let n = |k: &str| num(u, k).unwrap_or(0.0) as u32;
            s.utc = Some(format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                n("Year"),
                n("Month"),
                n("Day"),
                n("Hour"),
                n("Minute"),
                n("Second")
            ));
        }
        if let Ok(n) = self.dev(t, "GetNTP", "").await {
            s.ntp_dhcp = text(&n, "FromDHCP") == "true";
            let sec = if s.ntp_dhcp {
                "NTPFromDHCP"
            } else {
                "NTPManual"
            };
            s.ntp = elements(&n, sec)
                .into_iter()
                .map(|(_, v)| {
                    let d = text(v, "DNSname");
                    if d.is_empty() {
                        text(v, "IPv4Address")
                    } else {
                        d
                    }
                })
                .filter(|v| !v.is_empty())
                .collect();
        }
        Ok(s)
    }

    /// Time server, time zone, and optionally set the clock to `now` (UTC).
    pub async fn set_time(
        &self,
        t: &Target,
        s: &TimeSettings,
        now: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<()> {
        use chrono::{Datelike, Timelike};
        let servers: Vec<&str> = s
            .ntp
            .iter()
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .collect();
        if !s.ntp_dhcp && !servers.is_empty() {
            let body: String = servers
                .iter()
                .map(|v| {
                    if v.parse::<std::net::Ipv4Addr>().is_ok() {
                        format!("<NTPManual><tt:Type>IPv4</tt:Type><tt:IPv4Address>{}</tt:IPv4Address></NTPManual>", xml_escape(v))
                    } else {
                        format!("<NTPManual><tt:Type>DNS</tt:Type><tt:DNSname>{}</tt:DNSname></NTPManual>", xml_escape(v))
                    }
                })
                .collect();
            self.dev(t, "SetNTP", &format!("<FromDHCP>false</FromDHCP>{body}"))
                .await
                .map_err(|e| anyhow!("time server: {e:#}"))?;
        } else if s.ntp_dhcp {
            self.dev(t, "SetNTP", "<FromDHCP>true</FromDHCP>")
                .await
                .map_err(|e| anyhow!("time server: {e:#}"))?;
        }
        let manual = now.is_some();
        let mode = if manual {
            "Manual"
        } else if s.mode == "Manual" {
            "Manual"
        } else {
            "NTP"
        };
        let tz = if s.tz.trim().is_empty() {
            String::new()
        } else {
            format!(
                "<TimeZone><tt:TZ>{}</tt:TZ></TimeZone>",
                xml_escape(s.tz.trim())
            )
        };
        let clock = now
            .map(|d| {
                format!(
                    "<UTCDateTime><tt:Time><tt:Hour>{}</tt:Hour><tt:Minute>{}</tt:Minute><tt:Second>{}</tt:Second></tt:Time><tt:Date><tt:Year>{}</tt:Year><tt:Month>{}</tt:Month><tt:Day>{}</tt:Day></tt:Date></UTCDateTime>",
                    d.hour(), d.minute(), d.second(), d.year(), d.month(), d.day()
                )
            })
            .unwrap_or_default();
        self.dev(
            t,
            "SetSystemDateAndTime",
            &format!("<DateTimeType>{mode}</DateTimeType><DaylightSavings>false</DaylightSavings>{tz}{clock}"),
        )
        .await
        .map_err(|e| anyhow!("date & time: {e:#}"))?;
        // Back to the time server after setting the clock, if one is set.
        if manual && s.mode == "NTP" {
            let _ = self
                .dev(t, "SetSystemDateAndTime", &format!("<DateTimeType>NTP</DateTimeType><DaylightSavings>false</DaylightSavings>{tz}"))
                .await;
        }
        Ok(())
    }

    pub async fn reboot(&self, t: &Target) -> Result<String> {
        let r = self.dev(t, "SystemReboot", "").await?;
        Ok(text(&r, "Message"))
    }

    // ------------------------------------------------------------ video

    async fn video(&self, t: &Target, sv: &Services) -> Result<Vec<VideoStream>> {
        if let Some(m2) = &sv.media2
            && let Ok(v) = self.video_media2(t, m2).await
            && !v.is_empty()
        {
            return Ok(v);
        }
        let m1 = sv
            .media1
            .as_ref()
            .ok_or_else(|| anyhow!("the camera has no ONVIF media service"))?;
        self.video_media1(t, m1).await
    }

    async fn video_media2(&self, t: &Target, url: &str) -> Result<Vec<VideoStream>> {
        let x = self
            .call(
                t,
                url,
                &format!("{MEDIA2}/GetVideoEncoderConfigurations"),
                &format!(r#"<GetVideoEncoderConfigurations xmlns="{MEDIA2}"/>"#),
            )
            .await?;
        let mut out = Vec::new();
        for (tag, c) in elements(&x, "Configurations") {
            let token = attr(tag, "token").unwrap_or_default();
            let mut v = VideoStream {
                token: token.clone(),
                name: text(c, "Name"),
                encoding: text(c, "Encoding"),
                width: num(c, "Width").unwrap_or(0.0) as u32,
                height: num(c, "Height").unwrap_or(0.0) as u32,
                fps: num(c, "FrameRateLimit").unwrap_or(0.0),
                bitrate: num(c, "BitrateLimit").unwrap_or(0.0) as u32,
                gov: attr(tag, "GovLength")
                    .and_then(|g| g.parse().ok())
                    .unwrap_or(0),
                service: "media2".into(),
                ..Default::default()
            };
            if let Ok(o) = self
                .call(
                    t,
                    url,
                    &format!("{MEDIA2}/GetVideoEncoderConfigurationOptions"),
                    &format!(r#"<GetVideoEncoderConfigurationOptions xmlns="{MEDIA2}"><ConfigurationToken>{}</ConfigurationToken></GetVideoEncoderConfigurationOptions>"#, xml_escape(&token)),
                )
                .await
            {
                for (otag, ob) in elements(&o, "Options") {
                    let enc = text(ob, "Encoding");
                    if !enc.is_empty() && !v.encodings.contains(&enc) {
                        v.encodings.push(enc.clone());
                    }
                    if enc != v.encoding && !v.resolutions.is_empty() {
                        continue;
                    }
                    if enc == v.encoding || v.resolutions.is_empty() {
                        v.resolutions = elements(ob, "ResolutionsAvailable")
                            .into_iter()
                            .map(|(_, r)| (num(r, "Width").unwrap_or(0.0) as u32, num(r, "Height").unwrap_or(0.0) as u32))
                            .filter(|r| r.0 > 0)
                            .collect();
                        if let Some(f) = attr(otag, "FrameRatesSupported") {
                            v.fps_options = f.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                        }
                        if let Some((_, b)) = elements(ob, "BitrateRange").first() {
                            v.bitrate_min = num(b, "Min").unwrap_or(0.0) as u32;
                            v.bitrate_max = num(b, "Max").unwrap_or(0.0) as u32;
                        }
                    }
                }
            }
            out.push(v);
        }
        Ok(out)
    }

    async fn video_media1(&self, t: &Target, url: &str) -> Result<Vec<VideoStream>> {
        let x = self
            .call(
                t,
                url,
                &format!("{MEDIA1}/GetVideoEncoderConfigurations"),
                &format!(r#"<GetVideoEncoderConfigurations xmlns="{MEDIA1}"/>"#),
            )
            .await?;
        let mut out = Vec::new();
        for (tag, c) in elements(&x, "Configurations") {
            let token = attr(tag, "token").unwrap_or_default();
            let encoding = text(c, "Encoding");
            let mut v = VideoStream {
                token: token.clone(),
                name: text(c, "Name"),
                width: num(c, "Width").unwrap_or(0.0) as u32,
                height: num(c, "Height").unwrap_or(0.0) as u32,
                fps: num(c, "FrameRateLimit").unwrap_or(0.0),
                bitrate: num(c, "BitrateLimit").unwrap_or(0.0) as u32,
                gov: num(c, "GovLength").unwrap_or(0.0) as u32,
                encodings: vec![encoding.clone()],
                encoding,
                service: "media1".into(),
                ..Default::default()
            };
            if let Ok(o) = self
                .call(
                    t,
                    url,
                    &format!("{MEDIA1}/GetVideoEncoderConfigurationOptions"),
                    &format!(r#"<GetVideoEncoderConfigurationOptions xmlns="{MEDIA1}"><ConfigurationToken>{}</ConfigurationToken></GetVideoEncoderConfigurationOptions>"#, xml_escape(&token)),
                )
                .await
            {
                let sec = elements(&o, &v.encoding).into_iter().next().map(|e| e.1).unwrap_or(&o);
                v.resolutions = elements(sec, "ResolutionsAvailable")
                    .into_iter()
                    .map(|(_, r)| (num(r, "Width").unwrap_or(0.0) as u32, num(r, "Height").unwrap_or(0.0) as u32))
                    .filter(|r| r.0 > 0)
                    .collect();
                if let Some((_, f)) = elements(sec, "FrameRateRange").first() {
                    v.fps_min = num(f, "Min").unwrap_or(1.0);
                    v.fps_max = num(f, "Max").unwrap_or(30.0);
                }
                if let Some((_, b)) = elements(&o, "BitrateRange").first() {
                    v.bitrate_min = num(b, "Min").unwrap_or(0.0) as u32;
                    v.bitrate_max = num(b, "Max").unwrap_or(0.0) as u32;
                }
            }
            out.push(v);
        }
        Ok(out)
    }

    /// Change one video stream. The camera's own configuration is read,
    /// the chosen values are put in, and it is sent back whole (cameras
    /// reject partial configurations).
    pub async fn set_video(&self, t: &Target, ch: &VideoChange) -> Result<()> {
        let sv = self.services(t).await?;
        let (url, ns) = match (&sv.media2, &sv.media1) {
            (Some(m2), _)
                if self
                    .video_media2(t, m2)
                    .await
                    .is_ok_and(|v| v.iter().any(|x| x.token == ch.token)) =>
            {
                (m2.clone(), MEDIA2)
            }
            (_, Some(m1)) => (m1.clone(), MEDIA1),
            _ => bail!("the camera has no ONVIF media service"),
        };
        let x = self
            .call(
                t,
                &url,
                &format!("{ns}/GetVideoEncoderConfigurations"),
                &format!(r#"<GetVideoEncoderConfigurations xmlns="{ns}"/>"#),
            )
            .await?;
        let (tag, inner) = elements(&x, "Configurations")
            .into_iter()
            .find(|(tag, _)| attr(tag, "token").as_deref() == Some(ch.token.as_str()))
            .ok_or_else(|| anyhow!("stream {} not found on the camera", ch.token))?;
        let mut c = inner.to_string();
        c = set_text(&c, "Width", &ch.width.to_string());
        c = set_text(&c, "Height", &ch.height.to_string());
        if ch.fps > 0.0 {
            c = set_text(&c, "FrameRateLimit", &format!("{}", ch.fps));
        }
        if ch.bitrate > 0 {
            c = set_text(&c, "BitrateLimit", &ch.bitrate.to_string());
        }
        if !ch.encoding.is_empty() && ns == MEDIA2 {
            c = set_text(&c, "Encoding", &ch.encoding);
        }
        // Attributes of the configuration (token, GovLength, Profile …).
        let mut attrs: String = tag
            .split_whitespace()
            .skip(1)
            .filter(|a| !a.starts_with("xmlns"))
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches('/')
            .to_string();
        if ch.gov > 0 {
            if ns == MEDIA2 {
                if attrs.contains("GovLength=\"") {
                    let i = attrs.find("GovLength=\"").unwrap() + 11;
                    let j = attrs[i..].find('"').unwrap() + i;
                    attrs.replace_range(i..j, &ch.gov.to_string());
                } else {
                    attrs.push_str(&format!(" GovLength=\"{}\"", ch.gov));
                }
            } else {
                c = set_text(&c, "GovLength", &ch.gov.to_string());
            }
        }
        let body = format!(
            r#"<SetVideoEncoderConfiguration xmlns="{ns}"><Configuration {attrs}{nss}>{c}</Configuration>{persist}</SetVideoEncoderConfiguration>"#,
            nss = namespaces(&x),
            persist = if ns == MEDIA1 {
                "<ForcePersistence>true</ForcePersistence>"
            } else {
                ""
            }
        );
        self.call(
            t,
            &url,
            &format!("{ns}/SetVideoEncoderConfiguration"),
            &body,
        )
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------ picture

    async fn source_token(&self, t: &Target, sv: &Services) -> Result<String> {
        if let Some(m1) = &sv.media1
            && let Ok(x) = self
                .call(
                    t,
                    m1,
                    &format!("{MEDIA1}/GetVideoSources"),
                    &format!(r#"<GetVideoSources xmlns="{MEDIA1}"/>"#),
                )
                .await
            && let Some(tok) = attr_values(&x, "VideoSources", "token").into_iter().next()
        {
            return Ok(tok);
        }
        if let Some(m2) = &sv.media2
            && let Ok(x) = self
                .call(
                    t,
                    m2,
                    &format!("{MEDIA2}/GetVideoSourceConfigurations"),
                    &format!(r#"<GetVideoSourceConfigurations xmlns="{MEDIA2}"/>"#),
                )
                .await
        {
            let s = text(&x, "SourceToken");
            if !s.is_empty() {
                return Ok(s);
            }
        }
        bail!("the camera reported no video source")
    }

    async fn imaging(&self, t: &Target, sv: &Services) -> Result<Imaging> {
        let url = sv
            .imaging
            .as_ref()
            .ok_or_else(|| anyhow!("the camera has no ONVIF picture (imaging) settings"))?;
        let source = self.source_token(t, sv).await?;
        let x = self
            .call(t, url, &format!("{IMAGING}/GetImagingSettings"), &format!(r#"<GetImagingSettings xmlns="{IMAGING}"><VideoSourceToken>{}</VideoSourceToken></GetImagingSettings>"#, xml_escape(&source)))
            .await?;
        let settings = elements(&x, "ImagingSettings")
            .first()
            .map(|e| e.1)
            .unwrap_or(&x);
        let mut im = Imaging {
            source: source.clone(),
            brightness: num(settings, "Brightness"),
            contrast: num(settings, "Contrast"),
            saturation: num(settings, "ColorSaturation"),
            sharpness: num(settings, "Sharpness"),
            ir_cut: tag_text(settings, "IrCutFilter"),
            ..Default::default()
        };
        if let Ok(o) = self
            .call(t, url, &format!("{IMAGING}/GetOptions"), &format!(r#"<GetOptions xmlns="{IMAGING}"><VideoSourceToken>{}</VideoSourceToken></GetOptions>"#, xml_escape(&source)))
            .await
        {
            for (k, key) in [("Brightness", "brightness"), ("Contrast", "contrast"), ("ColorSaturation", "saturation"), ("Sharpness", "sharpness")] {
                if let Some((_, r)) = elements(&o, k).first()
                    && let (Some(a), Some(b)) = (num(r, "Min"), num(r, "Max"))
                {
                    im.ranges.insert(key.into(), (a, b));
                }
            }
            im.ir_modes = elements(&o, "IrCutFilterModes").into_iter().map(|(_, v)| v.trim().to_string()).collect();
        }
        Ok(im)
    }

    pub async fn set_imaging(&self, t: &Target, ch: &ImagingChange) -> Result<()> {
        let sv = self.services(t).await?;
        let url = sv
            .imaging
            .clone()
            .ok_or_else(|| anyhow!("the camera has no ONVIF picture (imaging) settings"))?;
        let source = self.source_token(t, &sv).await?;
        // Schema order matters: Brightness, ColorSaturation, Contrast, IrCutFilter, Sharpness.
        let mut s = String::new();
        let mut add = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                s.push_str(&format!("<tt:{k}>{}</tt:{k}>", xml_escape(&v)));
            }
        };
        add("Brightness", ch.brightness.map(|v| v.to_string()));
        add("ColorSaturation", ch.saturation.map(|v| v.to_string()));
        add("Contrast", ch.contrast.map(|v| v.to_string()));
        add("IrCutFilter", ch.ir_cut.clone().filter(|v| !v.is_empty()));
        add("Sharpness", ch.sharpness.map(|v| v.to_string()));
        self.call(
            t,
            &url,
            &format!("{IMAGING}/SetImagingSettings"),
            &format!(r#"<SetImagingSettings xmlns="{IMAGING}"><VideoSourceToken>{}</VideoSourceToken><ImagingSettings>{s}</ImagingSettings><ForcePersistence>true</ForcePersistence></SetImagingSettings>"#, xml_escape(&source)),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_helpers() {
        let x = r#"<env:Envelope xmlns:env="e" xmlns:tt="http://www.onvif.org/ver10/schema"><tds:NetworkInterfaces token="eth0"><tt:IPv4><tt:Config><tt:Manual><tt:Address>192.168.1.64</tt:Address><tt:PrefixLength>24</tt:PrefixLength></tt:Manual><tt:DHCP>false</tt:DHCP></tt:Config></tt:IPv4></tds:NetworkInterfaces></env:Envelope>"#;
        let e = elements(x, "NetworkInterfaces");
        assert_eq!(e.len(), 1);
        assert_eq!(attr(e[0].0, "token").as_deref(), Some("eth0"));
        let m = elements(e[0].1, "Manual");
        assert_eq!(text(m[0].1, "Address"), "192.168.1.64");
        assert_eq!(
            set_text("<a><tt:Width>640</tt:Width></a>", "Width", "1280"),
            "<a><tt:Width>1280</tt:Width></a>"
        );
        assert!(namespaces(x).contains(r#"xmlns:tt="http://www.onvif.org/ver10/schema""#));
    }
}
