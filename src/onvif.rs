//! Minimal ONVIF client for pan / tilt / zoom.
//!
//! ONVIF is the standard control protocol nearly all IP cameras support
//! (Hikvision, Dahua, Reolink, Tapo, Amcrest, Uniview, Axis, …). We use:
//! GetCapabilities (find the Media and PTZ services), GetProfiles (the
//! profile token), ContinuousMove / Stop, and presets.
//! Authentication is WS-Security UsernameToken with a password digest.

use crate::config::{CameraConfig, PtzConfig};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Preset {
    pub token: String,
    pub name: String,
}

/// Where to reach the camera and how to log in.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
}

impl Target {
    /// From the PTZ settings, filling blanks from the camera's stream URL.
    pub fn from(cam: &CameraConfig, ptz: &PtzConfig) -> Result<Self> {
        let parsed = parse_url(&cam.url);
        let host = if ptz.host.trim().is_empty() {
            parsed.as_ref().map(|p| p.0.clone()).unwrap_or_default()
        } else {
            ptz.host.trim().to_string()
        };
        if host.is_empty() {
            bail!("enter the camera's address for PTZ");
        }
        let user = if ptz.username.is_empty() {
            parsed.as_ref().map(|p| p.1.clone()).unwrap_or_default()
        } else {
            ptz.username.clone()
        };
        let pass = if ptz.password.is_empty() {
            parsed.as_ref().map(|p| p.2.clone()).unwrap_or_default()
        } else {
            ptz.password.clone()
        };
        Ok(Self {
            host,
            port: ptz.port,
            user,
            pass,
        })
    }

    fn device_url(&self) -> String {
        format!("http://{}:{}/onvif/device_service", self.host, self.port)
    }
}

/// (host, user, password) from `rtsp://user:pass@host:port/path`.
fn parse_url(url: &str) -> Option<(String, String, String)> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (auth, hostport) = match authority.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, authority),
    };
    let host = if hostport.starts_with('[') {
        hostport
            .split(']')
            .next()?
            .trim_start_matches('[')
            .to_string()
    } else {
        hostport.split(':').next()?.to_string()
    };
    let (user, pass) = match auth {
        Some(a) => {
            let (u, p) = a.split_once(':').unwrap_or((a, ""));
            (pct_decode(u), pct_decode(p))
        }
        None => (String::new(), String::new()),
    };
    Some((host, user, pass))
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// WS-Security header: PasswordDigest = Base64(SHA1(nonce + created + password)).
pub fn security_header(user: &str, pass: &str, nonce: &[u8], created: &str) -> String {
    if user.is_empty() {
        return String::new();
    }
    let mut h = Sha1::new();
    h.update(nonce);
    h.update(created.as_bytes());
    h.update(pass.as_bytes());
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        r#"<s:Header><Security s:mustUnderstand="1" xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"><UsernameToken><Username>{}</Username><Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{}</Password><Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{}</Nonce><Created xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">{}</Created></UsernameToken></Security></s:Header>"#,
        xml_escape(user),
        b64.encode(h.finalize()),
        b64.encode(nonce),
        created
    )
}

fn new_nonce() -> Vec<u8> {
    let mut h = Sha1::new();
    h.update(
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_le_bytes(),
    );
    h.update(NONCE_COUNTER.fetch_add(1, Ordering::SeqCst).to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.finalize()[..16].to_vec()
}

/// Text of the first element whose local name is `name` (any namespace prefix).
pub fn tag_text(xml: &str, name: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(i) = xml[pos..].find('<') {
        let start = pos + i + 1;
        let end = xml[start..].find('>')? + start;
        let tag = &xml[start..end];
        let tname = tag.split_whitespace().next().unwrap_or("");
        let local = tname.rsplit(':').next().unwrap_or(tname);
        if local == name && !tag.starts_with('/') && !tag.ends_with('/') {
            let close = xml[end + 1..].find("</")? + end + 1;
            return Some(xml[end + 1..close].trim().to_string());
        }
        pos = end + 1;
    }
    None
}

/// Value of attribute `attr` on every element with local name `name`.
pub fn attr_values(xml: &str, name: &str, attr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = xml[pos..].find('<') {
        let start = pos + i + 1;
        let Some(e) = xml[start..].find('>') else {
            break;
        };
        let end = e + start;
        let tag = &xml[start..end];
        let tname = tag.split_whitespace().next().unwrap_or("");
        let local = tname.rsplit(':').next().unwrap_or(tname);
        if local == name {
            let key = format!("{attr}=\"");
            if let Some(a) = tag.find(&key) {
                let v = &tag[a + key.len()..];
                if let Some(q) = v.find('"') {
                    out.push(v[..q].to_string());
                }
            }
        }
        pos = end + 1;
    }
    out
}

/// The `XAddr` inside the capability block `section` (e.g. "PTZ", "Media").
pub fn capability_xaddr(xml: &str, section: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(i) = xml[pos..].find('<') {
        let start = pos + i + 1;
        let end = xml[start..].find('>')? + start;
        let tag = &xml[start..end];
        let tname = tag.split_whitespace().next().unwrap_or("");
        let local = tname.rsplit(':').next().unwrap_or(tname);
        if local == section && !tag.starts_with('/') && !tag.ends_with('/') {
            return tag_text(&xml[end + 1..], "XAddr");
        }
        pos = end + 1;
    }
    None
}

#[derive(Default, Clone)]
struct Discovered {
    target: Option<Target>,
    ptz: String,
    profile: String,
}

pub struct Onvif {
    client: reqwest::Client,
    cache: Mutex<Discovered>,
}

impl Default for Onvif {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(Discovered::default()),
        }
    }
}

impl Onvif {
    async fn call(&self, t: &Target, url: &str, action: &str, body: &str) -> Result<String> {
        let created = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let header = security_header(&t.user, &t.pass, &new_nonce(), &created);
        let env = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tt="http://www.onvif.org/ver10/schema">{header}<s:Body>{body}</s:Body></s:Envelope>"#
        );
        let resp = self
            .client
            .post(url)
            .header(
                "Content-Type",
                format!("application/soap+xml; charset=utf-8; action=\"{action}\""),
            )
            .body(env)
            .send()
            .await
            .with_context(|| format!("cannot reach the camera's ONVIF service at {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let reason = tag_text(&text, "Text")
                .or_else(|| tag_text(&text, "Reason"))
                .unwrap_or_else(|| status.to_string());
            if status.as_u16() == 401
                || reason.to_lowercase().contains("not authorized")
                || reason.to_lowercase().contains("sender not authorized")
            {
                bail!(
                    "the camera refused the login ({reason}). Check the user name and password, and that the camera's clock is correct"
                );
            }
            bail!("camera replied: {reason}");
        }
        Ok(text)
    }

    /// Find the services and a PTZ-capable profile (cached).
    async fn discover(&self, t: &Target) -> Result<Discovered> {
        {
            let c = self.cache.lock().unwrap();
            if c.target.as_ref() == Some(t) && !c.profile.is_empty() {
                return Ok(c.clone());
            }
        }
        let caps = self
            .call(
                t,
                &t.device_url(),
                "http://www.onvif.org/ver10/device/wsdl/GetCapabilities",
                r#"<GetCapabilities xmlns="http://www.onvif.org/ver10/device/wsdl"><Category>All</Category></GetCapabilities>"#,
            )
            .await?;
        let media = capability_xaddr(&caps, "Media").unwrap_or_else(|| t.device_url());
        let ptz = capability_xaddr(&caps, "PTZ")
            .ok_or_else(|| anyhow!("this camera does not report PTZ support over ONVIF"))?;
        // Cameras sometimes report their internal address; keep their path but use the address we reach them on.
        let fix = |u: &str| -> String {
            match u
                .split_once("://")
                .and_then(|(_, r)| r.find('/').map(|i| r[i..].to_string()))
            {
                Some(path) => format!("http://{}:{}{}", t.host, t.port, path),
                None => u.to_string(),
            }
        };
        let (media, ptz) = (fix(&media), fix(&ptz));
        let profiles = self
            .call(
                t,
                &media,
                "http://www.onvif.org/ver10/media/wsdl/GetProfiles",
                r#"<GetProfiles xmlns="http://www.onvif.org/ver10/media/wsdl"/>"#,
            )
            .await?;
        let tokens = attr_values(&profiles, "Profiles", "token");
        let profile = tokens
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("the camera has no media profiles"))?;
        let d = Discovered {
            target: Some(t.clone()),
            ptz,
            profile,
        };
        *self.cache.lock().unwrap() = d.clone();
        Ok(d)
    }

    /// Start moving: pan, tilt, zoom are speeds from -1 to 1.
    pub async fn move_continuous(&self, t: &Target, pan: f32, tilt: f32, zoom: f32) -> Result<()> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<ContinuousMove xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken><Velocity><tt:PanTilt x="{:.2}" y="{:.2}"/><tt:Zoom x="{:.2}"/></Velocity></ContinuousMove>"#,
            xml_escape(&d.profile),
            pan.clamp(-1.0, 1.0),
            tilt.clamp(-1.0, 1.0),
            zoom.clamp(-1.0, 1.0)
        );
        self.call(
            t,
            &d.ptz,
            "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove",
            &body,
        )
        .await?;
        Ok(())
    }

    pub async fn stop(&self, t: &Target) -> Result<()> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<Stop xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken><PanTilt>true</PanTilt><Zoom>true</Zoom></Stop>"#,
            xml_escape(&d.profile)
        );
        self.call(t, &d.ptz, "http://www.onvif.org/ver20/ptz/wsdl/Stop", &body)
            .await?;
        Ok(())
    }

    pub async fn presets(&self, t: &Target) -> Result<Vec<Preset>> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<GetPresets xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken></GetPresets>"#,
            xml_escape(&d.profile)
        );
        let xml = self
            .call(
                t,
                &d.ptz,
                "http://www.onvif.org/ver20/ptz/wsdl/GetPresets",
                &body,
            )
            .await?;
        Ok(parse_presets(&xml))
    }

    pub async fn goto_preset(&self, t: &Target, token: &str) -> Result<()> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<GotoPreset xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken><PresetToken>{}</PresetToken></GotoPreset>"#,
            xml_escape(&d.profile),
            xml_escape(token)
        );
        self.call(
            t,
            &d.ptz,
            "http://www.onvif.org/ver20/ptz/wsdl/GotoPreset",
            &body,
        )
        .await?;
        Ok(())
    }

    /// Save the current position as a preset.
    pub async fn set_preset(&self, t: &Target, name: &str) -> Result<()> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<SetPreset xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken><PresetName>{}</PresetName></SetPreset>"#,
            xml_escape(&d.profile),
            xml_escape(name)
        );
        self.call(
            t,
            &d.ptz,
            "http://www.onvif.org/ver20/ptz/wsdl/SetPreset",
            &body,
        )
        .await?;
        Ok(())
    }

    pub async fn remove_preset(&self, t: &Target, token: &str) -> Result<()> {
        let d = self.discover(t).await?;
        let body = format!(
            r#"<RemovePreset xmlns="http://www.onvif.org/ver20/ptz/wsdl"><ProfileToken>{}</ProfileToken><PresetToken>{}</PresetToken></RemovePreset>"#,
            xml_escape(&d.profile),
            xml_escape(token)
        );
        self.call(
            t,
            &d.ptz,
            "http://www.onvif.org/ver20/ptz/wsdl/RemovePreset",
            &body,
        )
        .await?;
        Ok(())
    }
}

/// Presets from a GetPresetsResponse.
pub fn parse_presets(xml: &str) -> Vec<Preset> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("Preset ").or_else(|| rest.find("Preset\t")) {
        // Find the start of this element (the '<' before "...:Preset").
        let lt = rest[..i].rfind('<').unwrap_or(0);
        let tag_end = match rest[i..].find('>') {
            Some(e) => i + e,
            None => break,
        };
        let tag = &rest[lt..tag_end];
        let token = attr_values(
            &format!("{tag}>"),
            tag[1..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .rsplit(':')
                .next()
                .unwrap_or(""),
            "token",
        )
        .into_iter()
        .next();
        let after = &rest[tag_end..];
        let close = after.find("Preset>").unwrap_or(after.len());
        let name = tag_text(&after[..close], "Name").unwrap_or_default();
        if let Some(token) = token {
            let name = if name.is_empty() {
                format!("Preset {token}")
            } else {
                name
            };
            out.push(Preset { token, name });
        }
        rest = &rest[tag_end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_onvif_example() {
        // Same computation as the WS-Security spec: SHA1(nonce + created + password).
        let nonce = b"0123456789abcdef";
        let h = security_header("admin", "secret", nonce, "2026-01-01T00:00:00.000Z");
        let mut s = Sha1::new();
        s.update(nonce);
        s.update(b"2026-01-01T00:00:00.000Z");
        s.update(b"secret");
        let want = base64::engine::general_purpose::STANDARD.encode(s.finalize());
        assert!(h.contains(&format!("PasswordDigest\">{want}</Password>")));
        assert!(h.contains("<Username>admin</Username>"));
        assert!(security_header("", "", nonce, "x").is_empty());
    }

    #[test]
    fn parses_replies() {
        let caps = r#"<s:Envelope><s:Body><tds:GetCapabilitiesResponse><tds:Capabilities>
            <tt:Device><tt:XAddr>http://10.0.0.5/onvif/device_service</tt:XAddr></tt:Device>
            <tt:Media><tt:XAddr>http://10.0.0.5/onvif/Media</tt:XAddr></tt:Media>
            <tt:PTZ><tt:XAddr>http://10.0.0.5/onvif/PTZ</tt:XAddr></tt:PTZ></tds:Capabilities></tds:GetCapabilitiesResponse></s:Body></s:Envelope>"#;
        assert_eq!(
            capability_xaddr(caps, "PTZ").as_deref(),
            Some("http://10.0.0.5/onvif/PTZ")
        );
        assert_eq!(
            capability_xaddr(caps, "Media").as_deref(),
            Some("http://10.0.0.5/onvif/Media")
        );
        let profiles = r#"<trt:GetProfilesResponse><trt:Profiles fixed="true" token="Profile_1"><tt:Name>main</tt:Name></trt:Profiles><trt:Profiles token="Profile_2"/></trt:GetProfilesResponse>"#;
        assert_eq!(
            attr_values(profiles, "Profiles", "token"),
            ["Profile_1", "Profile_2"]
        );
        let presets = r#"<tptz:GetPresetsResponse><tptz:Preset token="1"><tt:Name>Gate</tt:Name><tt:PTZPosition/></tptz:Preset><tptz:Preset token="2"><tt:Name>Drive</tt:Name></tptz:Preset></tptz:GetPresetsResponse>"#;
        assert_eq!(
            parse_presets(presets),
            vec![
                Preset {
                    token: "1".into(),
                    name: "Gate".into()
                },
                Preset {
                    token: "2".into(),
                    name: "Drive".into()
                }
            ]
        );
        let fault = r#"<s:Fault><s:Reason><s:Text xml:lang="en">Sender not Authorized</s:Text></s:Reason></s:Fault>"#;
        assert_eq!(
            tag_text(fault, "Text").as_deref(),
            Some("Sender not Authorized")
        );
    }

    #[test]
    fn target_from_stream_url() {
        let cam = CameraConfig {
            url: "rtsp://admin:p%40ss@192.168.1.64:554/Streaming/Channels/101".into(),
            ..Default::default()
        };
        let t = Target::from(&cam, &PtzConfig::default()).unwrap();
        assert_eq!(
            t,
            Target {
                host: "192.168.1.64".into(),
                port: 80,
                user: "admin".into(),
                pass: "p@ss".into()
            }
        );
        let t = Target::from(
            &cam,
            &PtzConfig {
                host: "10.0.0.9".into(),
                port: 2020,
                username: "u".into(),
                password: "p".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.user.as_str()),
            ("10.0.0.9", 2020, "u")
        );
    }
}
