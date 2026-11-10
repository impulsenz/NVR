//! User accounts, sign-in sessions and what each user may do.
//!
//! Roles:
//! - **admin**: everything (settings, cameras, users).
//! - **operator**: watch everything, move PTZ cameras, pause recording,
//!   review and delete alerts, download clips.
//! - **viewer**: watch live view, alerts and recordings, download clips.
//!
//! A user can be limited to some cameras. Passwords are stored as Argon2
//! hashes; sessions are random tokens in a cookie, kept across restarts.

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
    sessions: Mutex<HashMap<String, Session>>,
    /// Failed sign-ins per user name (slows down password guessing).
    failures: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
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
    if p.chars().count() < 6 {
        bail!("passwords need at least 6 characters");
    }
    Ok(())
}

impl Users {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("users.json");
        let sessions_file = data_dir.join("sessions.json");
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
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    fn save_sessions(&self) {
        let s = self.sessions.lock().unwrap();
        if let Ok(b) = serde_json::to_vec(&*s) {
            let tmp = self.sessions_file.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
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
        if !self.setup_needed() {
            bail!("the admin account already exists");
        }
        self.create(username, password, Role::Admin, vec![])?;
        Ok(self.new_session(username.trim(), true))
    }

    pub fn create(
        &self,
        username: &str,
        password: &str,
        role: Role,
        cameras: Vec<String>,
    ) -> Result<()> {
        let name = check_name(username)?;
        check_password(password)?;
        {
            let mut u = self.users.write().unwrap();
            if u.iter().any(|x| x.username.eq_ignore_ascii_case(&name)) {
                bail!("there is already a user called \"{name}\"");
            }
            u.push(User {
                username: name,
                hash: hash(password)?,
                role,
                cameras,
                layouts: HashMap::new(),
                created: Some(Utc::now()),
            });
        }
        self.save()
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

    pub fn login(&self, username: &str, password: &str, remember: bool) -> Result<String> {
        let key = username.trim().to_lowercase();
        if let Some((n, at)) = self.failures.lock().unwrap().get(&key)
            && *n >= 5
            && Utc::now() - *at < Duration::seconds(30)
        {
            bail!("too many failed attempts — wait 30 seconds and try again");
        }
        let found = self
            .users
            .read()
            .unwrap()
            .iter()
            .find(|u| u.username.eq_ignore_ascii_case(username.trim()))
            .filter(|u| verify(password, &u.hash))
            .map(|u| u.username.clone());
        match found {
            Some(name) => {
                self.failures.lock().unwrap().remove(&key);
                Ok(self.new_session(&name, remember))
            }
            None => {
                let mut f = self.failures.lock().unwrap();
                let e = f.entry(key).or_insert((0, Utc::now()));
                e.0 += 1;
                e.1 = Utc::now();
                bail!("wrong user name or password")
            }
        }
    }

    fn new_session(&self, user: &str, remember: bool) -> String {
        let t = token();
        let days = if remember { 30 } else { 1 };
        self.sessions.lock().unwrap().insert(
            t.clone(),
            Session {
                user: user.to_string(),
                expires: Utc::now() + Duration::days(days),
            },
        );
        self.save_sessions();
        t
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
        self.save_sessions();
    }

    /// The user signed in with this session token.
    pub fn session_user(&self, token: &str) -> Option<User> {
        let name = {
            let s = self.sessions.lock().unwrap();
            let sess = s.get(token)?;
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
        let t = u.setup("Boss", "secret1").unwrap();
        assert_eq!(u.session_user(&t).unwrap().role, Role::Admin);
        assert!(u.setup("x", "secret1").is_err());
        u.create("kid", "secret2", Role::Viewer, vec!["front".into()])
            .unwrap();
        assert!(u.login("kid", "nope", false).is_err());
        let t2 = u.login("KID", "secret2", false).unwrap();
        let kid = u.session_user(&t2).unwrap();
        assert!(kid.can_see("front") && !kid.can_see("back"));
        assert!(u.delete("Boss").is_err(), "last admin stays");
        // Survives a restart.
        let u2 = Users::open(&dir);
        assert_eq!(u2.session_user(&t2).unwrap().username, "kid");
        u2.update("kid", None, None, Some("newpass")).unwrap();
        assert!(u2.session_user(&t2).is_none(), "password change signs out");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
