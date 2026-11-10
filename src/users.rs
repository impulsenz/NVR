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
}

impl User {
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
            })
            .collect()
    }

    /// Sign in from address `from`. Slow on purpose (Argon2): call from a
    /// blocking thread.
    pub fn login(
        &self,
        username: &str,
        password: &str,
        remember: bool,
        from: &str,
    ) -> Result<String> {
        let name = self.check(username, password, from)?;
        Ok(self.new_session(&name, remember))
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
        assert!(u.login("kid", "nope", false, "1.2.3.4").is_err());
        let t2 = u.login("KID", "secret23", false, "1.2.3.4").unwrap();
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
