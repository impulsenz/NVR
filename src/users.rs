//! User accounts, sign-in sessions and what each user may do.
//!
//! Roles:
//! - **admin**: everything (settings, cameras, users).
//! - **operator**: watch everything, move PTZ cameras, pause recording,
//!   review and delete alerts, download clips.
//! - **viewer**: watch live view, alerts and recordings, download clips.
//!
//! A user can be limited to some cameras. Passwords are stored as Argon2
//! hashes; sessions are random tokens in a cookie, kept across restarts
//! (only a SHA-256 of each token is stored, so the file can't be used to
//! sign in). Repeated wrong passwords lock out the user name and the
//! address they come from, for longer each time.

use crate::config::DashboardConfig;
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    #[serde(default)]
    hash: String,
    pub role: Role,
    /// Cameras this user may see (empty = all).
    #[serde(default)]
    pub cameras: Vec<String>,
    /// Personal dashboard layouts: "desktop" and "mobile".
    #[serde(default)]
    pub layouts: HashMap<String, DashboardConfig>,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    /// Two-step sign-in: the authenticator app's secret (base32).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    totp: Option<String>,
    /// One-time recovery codes (SHA-256 of each).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recovery: Vec<String>,
    /// API tokens: for scripts, Home Assistant or another GAZE (they act as
    /// this user). Only their SHA-256 is kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tokens: Vec<ApiToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    hash: String,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
}

impl User {
    pub fn two_step(&self) -> bool {
        self.totp.is_some()
    }
    pub fn can_see(&self, camera: &str) -> bool {
        self.cameras.is_empty() || self.cameras.iter().any(|c| c == camera)
    }
}

/// What the browser is told about the signed-in user.
#[derive(Debug, Clone, Serialize)]
pub struct Me {
    pub username: String,
    pub role: Role,
    pub cameras: Vec<String>,
    /// Two-step sign-in is on.
    pub two_step: bool,
}

#[derive(Serialize, Deserialize)]
struct Session {
    user: String,
    expires: DateTime<Utc>,
}

pub struct Users {
    file: PathBuf,
    sessions_file: PathBuf,
    users: RwLock<Vec<User>>,
    /// By SHA-256 of the session token.
    sessions: Mutex<HashMap<String, Session>>,
    /// Failed sign-ins per user name and per address (slows down guessing).
    failures: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
}

/// Signing in needs the code from the authenticator app (the page asks for it).
pub const NEED_CODE: &str = "two-step code needed";
static PENDING: std::sync::LazyLock<Mutex<HashMap<String, String>>> = std::sync::LazyLock::new(Default::default);
static LAST_STEP: std::sync::LazyLock<Mutex<HashMap<String, i64>>> = std::sync::LazyLock::new(Default::default);

/// RFC 6238 time-based code (HMAC-SHA1, 6 digits) for time step `step`.
pub fn totp(key: &[u8], step: u64) -> u32 {
    use sha1::{Digest, Sha1};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha1::new().chain_update(ipad).chain_update(step.to_be_bytes()).finalize();
    let mac = Sha1::new().chain_update(opad).chain_update(inner).finalize();
    let o = (mac[19] & 0x0f) as usize;
    let bin = ((mac[o] as u32 & 0x7f) << 24) | ((mac[o + 1] as u32) << 16) | ((mac[o + 2] as u32) << 8) | mac[o + 3] as u32;
    bin % 1_000_000
}

const B32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
pub fn base32_encode(b: &[u8]) -> String {
    let (mut out, mut buf, mut bits) = (String::new(), 0u32, 0);
    for &x in b {
        buf = (buf << 8) | x as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut buf, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = B32.iter().position(|&x| x == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Failed attempts allowed before a lock-out.
const FREE_TRIES: u32 = 5;

/// How long a key with `n` failures is locked: 30 s after 5, doubling
/// with each further failure, at most 15 minutes.
fn lockout(n: u32) -> Duration {
    if n < FREE_TRIES {
        return Duration::zero();
    }
    Duration::seconds((30i64 << (n - FREE_TRIES).min(5)).min(900))
}

fn digest(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Only the owner may read these files (passwords hashes, sessions).
fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Checked when the user name doesn't exist, so a wrong name takes as
/// long as a wrong password (no guessing which names exist).
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash("not a real password").unwrap_or_default())
}

fn hash(password: &str) -> Result<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand_core::OsRng);
    Ok(argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("{e}"))?
        .to_string())
}

fn verify(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

fn token() -> String {
    use rand_core::RngCore;
    let mut b = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn check_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 40 || n.contains(|c: char| c.is_control()) {
        bail!("user names are 1–40 characters");
    }
    Ok(n.to_string())
}

fn check_password(p: &str) -> Result<()> {
    if p.chars().count() < 8 {
        bail!("passwords need at least 8 characters");
    }
    if p.len() > 256 {
        bail!("that password is too long");
    }
    Ok(())
}

impl Users {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("users.json");
        let sessions_file = data_dir.join("sessions.json");
        private(&file);
        private(&sessions_file);
        let users: Vec<User> = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut sessions: HashMap<String, Session> = std::fs::read(&sessions_file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        sessions
            .retain(|_, s| s.expires > Utc::now() && users.iter().any(|u| u.username == s.user));
        Self {
            file,
            sessions_file,
            users: RwLock::new(users),
            sessions: Mutex::new(sessions),
            failures: Mutex::default(),
        }
    }

    fn save(&self) -> Result<()> {
        let b = serde_json::to_vec_pretty(&*self.users.read().unwrap())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, b)?;
        private(&tmp);
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    /// For a backup: every account with its role, cameras and layouts;
    /// with `secrets`, also passwords (as stored: hashed), two-step sign-in
    /// and API tokens.
    pub fn export(&self, secrets: bool) -> Vec<u8> {
        let mut users = self.users.read().unwrap().clone();
        if !secrets {
            for u in &mut users {
                u.hash.clear();
                u.totp = None;
                u.recovery.clear();
                u.tokens.clear();
            }
        }
        serde_json::to_vec_pretty(&users).unwrap_or_default()
    }

    /// From a backup: accounts here are updated (role, cameras, layouts —
    /// and the password etc. when the backup has them); new ones are added
    /// when the backup has their password. Returns (updated or added,
    /// skipped for want of a password).
    pub fn import(&self, json: &[u8]) -> Result<(usize, usize)> {
        let incoming: Vec<User> = serde_json::from_slice(json)?;
        let (mut n, mut skipped) = (0, 0);
        {
            let mut users = self.users.write().unwrap();
            for mut u in incoming {
                let Ok(name) = check_name(&u.username) else { continue };
                u.username = name;
                match users.iter_mut().find(|x| x.username.eq_ignore_ascii_case(&u.username)) {
                    Some(x) => {
                        x.role = u.role;
                        x.cameras = u.cameras;
                        x.layouts = u.layouts;
                        if !u.hash.is_empty() {
                            x.hash = u.hash;
                            x.totp = u.totp;
                            x.recovery = u.recovery;
                            x.tokens = u.tokens;
                        }
                        n += 1;
                    }
                    None if !u.hash.is_empty() => {
                        users.push(u);
                        n += 1;
                    }
                    None => skipped += 1,
                }
            }
        }
        self.save()?;
        Ok((n, skipped))
    }

    fn save_sessions(&self) {
        let s = self.sessions.lock().unwrap();
        if let Ok(b) = serde_json::to_vec(&*s) {
            let tmp = self.sessions_file.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                private(&tmp);
                let _ = std::fs::rename(&tmp, &self.sessions_file);
            }
        }
    }

    /// No accounts yet: the first visitor sets up the admin account.
    pub fn setup_needed(&self) -> bool {
        self.users.read().unwrap().is_empty()
    }

    /// Earlier versions had one optional login in the settings: it becomes
    /// the admin account.
    pub fn migrate_legacy(&self, username: &str, password: &str) {
        if !self.setup_needed() || username.trim().is_empty() {
            return;
        }
        if let Ok(h) = hash(password) {
            self.users.write().unwrap().push(User {
                username: username.trim().to_string(),
                hash: h,
                role: Role::Admin,
                cameras: vec![],
                layouts: HashMap::new(),
                created: Some(Utc::now()),
                totp: None,
                recovery: Vec::new(),
                tokens: Vec::new(),
            });
            let _ = self.save();
            tracing::info!(
                "the login from the settings is now the admin account \"{}\"",
                username.trim()
            );
        }
    }

    pub fn setup(&self, username: &str, password: &str) -> Result<String> {
        let name = self.add(username, password, Role::Admin, vec![], true)?;
        Ok(self.new_session(&name, true))
    }

    pub fn create(
        &self,
        username: &str,
        password: &str,
        role: Role,
        cameras: Vec<String>,
    ) -> Result<()> {
        self.add(username, password, role, cameras, false)
            .map(|_| ())
    }

    /// `first`: only while there are no accounts (first-time setup; checked
    /// under the same lock, so two visitors can't both become admin).
    fn add(
        &self,
        username: &str,
        password: &str,
        role: Role,
        cameras: Vec<String>,
        first: bool,
    ) -> Result<String> {
        let name = check_name(username)?;
        check_password(password)?;
        let h = hash(password)?;
        {
            let mut u = self.users.write().unwrap();
            if first && !u.is_empty() {
                bail!("the admin account already exists");
            }
            if u.iter().any(|x| x.username.eq_ignore_ascii_case(&name)) {
                bail!("there is already a user called \"{name}\"");
            }
            u.push(User {
                username: name.clone(),
                hash: h,
                role,
                cameras,
                layouts: HashMap::new(),
                created: Some(Utc::now()),
                totp: None,
                recovery: Vec::new(),
                tokens: Vec::new(),
            });
        }
        self.save()?;
        Ok(name)
    }

    pub fn update(
        &self,
        username: &str,
        role: Option<Role>,
        cameras: Option<Vec<String>>,
        password: Option<&str>,
    ) -> Result<()> {
        if let Some(p) = password {
            check_password(p)?;
        }
        {
            let mut all = self.users.write().unwrap();
            let admins = all.iter().filter(|u| u.role == Role::Admin).count();
            let u = all
                .iter_mut()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow!("no such user"))?;
            if let Some(r) = role {
                if u.role == Role::Admin && r != Role::Admin && admins <= 1 {
                    bail!("there must be at least one admin");
                }
                u.role = r;
            }
            if let Some(c) = cameras {
                u.cameras = c;
            }
            if let Some(p) = password {
                u.hash = hash(p)?;
            }
        }
        if password.is_some() {
            // Signed in elsewhere with the old password: signed out.
            self.sessions
                .lock()
                .unwrap()
                .retain(|_, s| s.user != username);
            self.save_sessions();
        }
        self.save()
    }

    pub fn delete(&self, username: &str) -> Result<()> {
        {
            let mut all = self.users.write().unwrap();
            let admins = all.iter().filter(|u| u.role == Role::Admin).count();
            let u = all
                .iter()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow!("no such user"))?;
            if u.role == Role::Admin && admins <= 1 {
                bail!("the last admin can't be deleted");
            }
            all.retain(|u| u.username != username);
        }
        self.sessions
            .lock()
            .unwrap()
            .retain(|_, s| s.user != username);
        self.save_sessions();
        self.save()
    }

    pub fn list(&self) -> Vec<Me> {
        self.users
            .read()
            .unwrap()
            .iter()
            .map(|u| Me {
                username: u.username.clone(),
                role: u.role,
                cameras: u.cameras.clone(),
                two_step: u.totp.is_some(),
            })
            .collect()
    }

    /// Sign in from address `from`. Slow on purpose (Argon2): call from a
    /// blocking thread.
    pub fn login(
        &self,
        username: &str,
        password: &str,
        code: Option<&str>,
        remember: bool,
        from: &str,
    ) -> Result<String> {
        let name = self.check(username, password, from)?;
        if self.has_two_step(&name) {
            let code = code.map(str::trim).filter(|c| !c.is_empty());
            let Some(code) = code else { bail!(NEED_CODE) };
            if !self.check_code(&name, code) {
                self.failed(&[format!("user:{}", name.to_lowercase()), format!("ip:{from}")]);
                bail!("that code isn't right — use the 6-digit code from your authenticator app now (or a recovery code)");
            }
        }
        Ok(self.new_session(&name, remember))
    }

    fn failed(&self, keys: &[String]) {
        let mut f = self.failures.lock().unwrap();
        for k in keys {
            let e = f.entry(k.clone()).or_insert((0, Utc::now()));
            e.0 += 1;
            e.1 = Utc::now();
        }
    }

    pub fn has_two_step(&self, name: &str) -> bool {
        self.users.read().unwrap().iter().any(|u| u.username == name && u.totp.is_some())
    }

    /// A code from the authenticator app (this 30 s, or the one before or
    /// after: phone clocks drift), or an unused recovery code.
    fn check_code(&self, name: &str, code: &str) -> bool {
        let mut users = self.users.write().unwrap();
        let Some(u) = users.iter_mut().find(|u| u.username == name) else { return false };
        let Some(secret) = u.totp.as_ref().and_then(|s| base32_decode(s)) else { return false };
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 6 {
            let step = Utc::now().timestamp() / 30;
            let mut last = LAST_STEP.lock().unwrap();
            let used = last.get(name).copied().unwrap_or(i64::MIN);
            for s in [step - 1, step, step + 1] {
                // Each code works once (no replaying one seen over a shoulder).
                if s > used && format!("{:06}", totp(&secret, s as u64)) == digits {
                    last.insert(name.to_string(), s);
                    return true;
                }
            }
            return false;
        }
        // A recovery code: works once.
        let h = sha256_hex(&code.to_lowercase().replace([' ', '-'], ""));
        if let Some(i) = u.recovery.iter().position(|r| *r == h) {
            u.recovery.remove(i);
            drop(users);
            let _ = self.save();
            return true;
        }
        false
    }

    /// Start turning on two-step sign-in: a new secret, not used until
    /// confirmed with a code (`finish_two_step`).
    pub fn start_two_step(&self, name: &str) -> (String, String) {
        use rand_core::RngCore;
        let mut k = [0u8; 20];
        rand_core::OsRng.fill_bytes(&mut k);
        let secret = base32_encode(&k);
        PENDING.lock().unwrap().insert(name.to_string(), secret.clone());
        let uri = format!("otpauth://totp/GAZE:{}?secret={secret}&issuer=GAZE&digits=6&period=30", pct(name));
        (secret, uri)
    }

    /// Confirm with a code from the app; returns the recovery codes (shown once).
    pub fn finish_two_step(&self, name: &str, code: &str) -> Result<Vec<String>> {
        let secret = PENDING.lock().unwrap().get(name).cloned().ok_or_else(|| anyhow::anyhow!("start again: scan the new QR code"))?;
        let key = base32_decode(&secret).unwrap_or_default();
        let step = Utc::now().timestamp() / 30;
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        if ![step - 1, step, step + 1].iter().any(|s| format!("{:06}", totp(&key, *s as u64)) == digits) {
            bail!("that code isn't right — check the phone's clock is set automatically, and type the code the app shows now");
        }
        use rand_core::RngCore;
        let codes: Vec<String> = (0..8)
            .map(|_| {
                let mut b = [0u8; 5];
                rand_core::OsRng.fill_bytes(&mut b);
                let c = base32_encode(&b).to_lowercase();
                format!("{}-{}", &c[..4], &c[4..8])
            })
            .collect();
        {
            let mut users = self.users.write().unwrap();
            let u = users.iter_mut().find(|u| u.username == name).ok_or_else(|| anyhow::anyhow!("no such user"))?;
            u.totp = Some(secret);
            u.recovery = codes.iter().map(|c| sha256_hex(&c.replace('-', ""))).collect();
        }
        PENDING.lock().unwrap().remove(name);
        self.save()?;
        Ok(codes)
    }

    /// Turn two-step sign-in off (the user, after their password; or an admin).
    pub fn stop_two_step(&self, name: &str) -> Result<()> {
        {
            let mut users = self.users.write().unwrap();
            let u = users.iter_mut().find(|u| u.username == name).ok_or_else(|| anyhow::anyhow!("no such user"))?;
            u.totp = None;
            u.recovery.clear();
        }
        self.save()
    }

    /// The user's name (as stored) if the password is right. Counts wrong
    /// passwords against the user name and the address.
    pub fn check(&self, username: &str, password: &str, from: &str) -> Result<String> {
        let keys = [
            format!("user:{}", username.trim().to_lowercase()),
            format!("ip:{from}"),
        ];
        {
            let f = self.failures.lock().unwrap();
            for k in &keys {
                if let Some((n, at)) = f.get(k) {
                    let wait = lockout(*n) - (Utc::now() - *at);
                    if wait > Duration::zero() {
                        bail!(
                            "too many failed attempts — wait {} seconds and try again",
                            wait.num_seconds().max(1)
                        );
                    }
                }
            }
        }
        let user = self
            .users
            .read()
            .unwrap()
            .iter()
            .find(|u| u.username.eq_ignore_ascii_case(username.trim()))
            .map(|u| (u.username.clone(), u.hash.clone()));
        // Always check a hash, so unknown names aren't faster.
        let ok = match &user {
            Some((_, h)) => verify(password, h),
            None => {
                verify(password, dummy_hash());
                false
            }
        };
        let mut f = self.failures.lock().unwrap();
        if ok {
            for k in &keys {
                f.remove(k);
            }
            return Ok(user.unwrap().0);
        }
        if f.len() > 10_000 {
            // Don't let random names fill memory; keep the locked-out ones.
            f.retain(|_, (n, at)| *n >= FREE_TRIES && Utc::now() - *at < Duration::minutes(15));
        }
        for k in keys {
            let e = f.entry(k).or_insert((0, Utc::now()));
            e.0 += 1;
            e.1 = Utc::now();
        }
        bail!("wrong user name or password")
    }

    pub fn new_session(&self, user: &str, remember: bool) -> String {
        let t = token();
        let days = if remember { 30 } else { 1 };
        {
            let mut s = self.sessions.lock().unwrap();
            s.retain(|_, x| x.expires > Utc::now());
            s.insert(
                digest(&t),
                Session {
                    user: user.to_string(),
                    expires: Utc::now() + Duration::days(days),
                },
            );
        }
        self.save_sessions();
        t
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap().remove(&digest(token));
        self.save_sessions();
    }

    /// The user signed in with this session token.
    /// The user an API token belongs to ("Authorization: Bearer …").
    pub fn token_user(&self, token: &str) -> Option<User> {
        let h = digest(token.trim());
        self.users.read().unwrap().iter().find(|u| u.tokens.iter().any(|t| t.hash == h)).cloned()
    }

    /// A new API token for this user: shown once, only its hash is kept.
    pub fn new_token(&self, user: &str, name: &str) -> Result<(String, String)> {
        let name = name.trim();
        if name.is_empty() || name.len() > 60 {
            bail!("give the token a name (what it's for), up to 60 characters");
        }
        let t = format!("gaze_{}", token());
        let id = token()[..12].to_string();
        {
            let mut users = self.users.write().unwrap();
            let u = users.iter_mut().find(|u| u.username == user).ok_or_else(|| anyhow::anyhow!("no such user"))?;
            if u.tokens.len() >= 20 {
                bail!("at most 20 tokens per user — remove one first");
            }
            u.tokens.push(ApiToken { id: id.clone(), name: name.to_string(), hash: digest(&t), created: Some(Utc::now()) });
        }
        self.save()?;
        Ok((id, t))
    }

    pub fn tokens(&self, user: &str) -> Vec<serde_json::Value> {
        self.users.read().unwrap().iter().find(|u| u.username == user)
            .map(|u| u.tokens.iter().map(|t| serde_json::json!({"id": t.id, "name": t.name, "created": t.created})).collect())
            .unwrap_or_default()
    }

    pub fn remove_token(&self, user: &str, id: &str) -> Result<()> {
        {
            let mut users = self.users.write().unwrap();
            let u = users.iter_mut().find(|u| u.username == user).ok_or_else(|| anyhow::anyhow!("no such user"))?;
            u.tokens.retain(|t| t.id != id);
        }
        self.save()
    }

    pub fn session_user(&self, token: &str) -> Option<User> {
        let name = {
            let s = self.sessions.lock().unwrap();
            let sess = s.get(&digest(token))?;
            if sess.expires < Utc::now() {
                return None;
            }
            sess.user.clone()
        };
        self.users
            .read()
            .unwrap()
            .iter()
            .find(|u| u.username == name)
            .cloned()
    }

    pub fn layout(&self, username: &str, device: &str) -> Option<DashboardConfig> {
        self.users
            .read()
            .unwrap()
            .iter()
            .find(|u| u.username == username)
            .and_then(|u| u.layouts.get(device).cloned())
    }

    pub fn set_layout(&self, username: &str, device: &str, d: DashboardConfig) -> Result<()> {
        {
            let mut all = self.users.write().unwrap();
            let u = all
                .iter_mut()
                .find(|u| u.username == username)
                .ok_or_else(|| anyhow!("no such user"))?;
            u.layouts.insert(device.to_string(), d);
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_step_codes_match_the_standard() {
        // RFC 6238, SHA-1 example: time 59 s → 94287082 (6 digits: 287082).
        let key = b"12345678901234567890";
        assert_eq!(totp(key, 59 / 30), 287082);
        assert_eq!(totp(key, 1111111109 / 30), 81804);
        assert_eq!(base32_decode(&base32_encode(key)).unwrap(), key.to_vec());
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn signing_in_with_two_step() {
        let dir = std::env::temp_dir().join(format!("nvr-2s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let u = Users::open(&dir);
        u.setup("sam", "a good password").unwrap();
        let (secret, uri) = u.start_two_step("sam");
        assert!(uri.starts_with("otpauth://totp/GAZE:sam?secret="));
        let key = base32_decode(&secret).unwrap();
        let now = || format!("{:06}", totp(&key, (Utc::now().timestamp() / 30) as u64));
        let codes = u.finish_two_step("sam", &now()).unwrap();
        assert_eq!(codes.len(), 8);
        // The password alone isn't enough any more.
        assert_eq!(u.login("sam", "a good password", None, false, "1.2.3.4").unwrap_err().to_string(), NEED_CODE);
        assert!(u.login("sam", "a good password", Some("000000"), false, "1.2.3.4").is_err() || now() == "000000");
        assert!(u.login("sam", "a good password", Some(&now()), false, "1.2.3.5").is_ok());
        // The same code doesn't work twice.
        assert!(u.login("sam", "a good password", Some(&now()), false, "1.2.3.6").is_err());
        // A recovery code works once.
        assert!(u.login("sam", "a good password", Some(&codes[0]), false, "1.2.3.7").is_ok());
        assert!(u.login("sam", "a good password", Some(&codes[0]), false, "1.2.3.8").is_err());
        u.stop_two_step("sam").unwrap();
        assert!(u.login("sam", "a good password", None, false, "1.2.3.9").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accounts_and_sessions() {
        let dir = std::env::temp_dir().join(format!("nvr-users-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let u = Users::open(&dir);
        assert!(u.setup_needed());
        assert!(
            u.setup("Boss", "short").is_err(),
            "passwords need 8 characters"
        );
        let t = u.setup("Boss", "secret12").unwrap();
        assert_eq!(u.session_user(&t).unwrap().role, Role::Admin);
        assert!(u.setup("x", "secret12").is_err());
        u.create("kid", "secret23", Role::Viewer, vec!["front".into()])
            .unwrap();
        assert!(u.login("kid", "nope", None, false, "1.2.3.4").is_err());
        let t2 = u.login("KID", "secret23", None, false, "1.2.3.4").unwrap();
        let kid = u.session_user(&t2).unwrap();
        assert!(kid.can_see("front") && !kid.can_see("back"));
        assert!(u.delete("Boss").is_err(), "last admin stays");
        // Survives a restart; the file holds no usable tokens.
        let file = std::fs::read_to_string(dir.join("sessions.json")).unwrap();
        assert!(!file.contains(&t2), "only token digests are stored");
        let u2 = Users::open(&dir);
        assert_eq!(u2.session_user(&t2).unwrap().username, "kid");
        u2.update("kid", None, None, Some("newpass1")).unwrap();
        assert!(u2.session_user(&t2).is_none(), "password change signs out");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guessing_is_locked_out() {
        let dir = std::env::temp_dir().join(format!("nvr-users-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let u = Users::open(&dir);
        u.setup("admin", "right-pass").unwrap();
        for _ in 0..FREE_TRIES {
            assert!(
                u.check("admin", "wrong", "9.9.9.9")
                    .unwrap_err()
                    .to_string()
                    .contains("wrong")
            );
        }
        // Locked: even the right password is refused for now…
        assert!(
            u.check("admin", "right-pass", "9.9.9.9")
                .unwrap_err()
                .to_string()
                .contains("wait")
        );
        // …and the address is locked for other names too.
        assert!(
            u.check("someone", "x", "9.9.9.9")
                .unwrap_err()
                .to_string()
                .contains("wait")
        );
        assert_eq!(lockout(5), Duration::seconds(30));
        assert_eq!(lockout(7), Duration::seconds(120));
        assert_eq!(lockout(50), Duration::seconds(900));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
