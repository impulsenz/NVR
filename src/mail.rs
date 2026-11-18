//! Email notifications (Settings → Notifications → Email): the same message
//! as the ntfy push, sent through an SMTP server. The alert picture is
//! attached. Nothing here touches recording; a mail server that is down only
//! loses the email (the error shows on the Notifications page).

use crate::config::EmailConfig;
use crate::notify::Message;
use anyhow::{Context, Result, bail};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use std::time::Duration;

/// `X-Priority: 1`: mail programs mark the message as urgent.
#[derive(Clone)]
struct XPriority;

impl lettre::message::header::Header for XPriority {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("X-Priority")
    }
    fn parse(_: &str) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(XPriority)
    }
    fn display(&self) -> lettre::message::header::HeaderValue {
        lettre::message::header::HeaderValue::new(Self::name(), "1 (Highest)".to_string())
    }
}

fn mailbox(s: &str) -> Result<Mailbox> {
    s.trim().parse().with_context(|| format!("\"{}\" is not a valid email address", s.trim()))
}

/// Build the email (separate from sending so it can be tested).
pub fn build(cfg: &EmailConfig, m: &Message<'_>) -> Result<lettre::Message> {
    let from = if cfg.from.trim().is_empty() { &cfg.username } else { &cfg.from };
    if from.trim().is_empty() {
        bail!("enter a From address");
    }
    let mut b = lettre::Message::builder().from(mailbox(from)?).subject(subject(m));
    for r in cfg.recipients() {
        b = b.to(mailbox(&r)?);
    }
    if m.priority >= 4 {
        b = b.header(XPriority);
    }
    let mut text = m.body.clone();
    if let Some(c) = &m.click {
        text.push_str(&format!("\n\nOpen in GAZE: {c}"));
    }
    let text = SinglePart::builder().header(ContentType::TEXT_PLAIN).body(text);
    let msg = match &m.picture {
        Some(p) => b.multipart(
            MultiPart::mixed()
                .singlepart(text)
                .singlepart(Attachment::new("alert.jpg".to_string()).body(p.clone(), "image/jpeg".parse().unwrap())),
        ),
        None => b.singlepart(text),
    };
    msg.context("could not build the email")
}

fn subject(m: &Message<'_>) -> String {
    // No line breaks in a header.
    let t = m.title.replace(['\r', '\n'], " ");
    if m.priority >= 4 { format!("⚠ {t}") } else { t }
}

pub async fn send(cfg: &EmailConfig, m: &Message<'_>) -> Result<()> {
    let email = build(cfg, m)?;
    let host = cfg.host.trim();
    let mut t = match cfg.security.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
        "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
    }
    .with_context(|| format!("cannot use the mail server \"{host}\""))?
    .port(cfg.port)
    .timeout(Some(Duration::from_secs(30)));
    if !cfg.username.trim().is_empty() {
        t = t.credentials(Credentials::new(cfg.username.trim().to_string(), cfg.password.clone()));
    }
    t.build().send(email).await.with_context(|| format!("sending through {host}:{}", cfg.port))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(priority: u8, picture: bool) -> Message<'static> {
        Message {
            title: "Front Door: Person\nsecond line".into(),
            body: "A person, 12 s".into(),
            tags: "",
            priority,
            click: Some("http://pc:8080/#a".into()),
            picture: picture.then(|| vec![0xff, 0xd8, 0xff]),
            actions: None,
        }
    }

    fn cfg() -> EmailConfig {
        EmailConfig { enabled: true, host: "smtp.example.com".into(), username: "me@example.com".into(), to: "a@example.com, b@example.com;".into(), ..Default::default() }
    }

    #[test]
    fn recipients_split_on_commas_and_semicolons() {
        assert_eq!(cfg().recipients(), vec!["a@example.com", "b@example.com"]);
        assert!(cfg().ready());
        assert!(!EmailConfig { enabled: false, ..cfg() }.ready());
        assert!(!EmailConfig { to: " , ".into(), ..cfg() }.ready());
    }

    #[test]
    fn builds_a_message_with_picture_and_safe_subject() {
        let e = build(&cfg(), &msg(3, true)).unwrap();
        let raw = String::from_utf8_lossy(&e.formatted()).to_string();
        assert!(raw.contains("multipart/mixed") && raw.contains("alert.jpg"));
        assert!(raw.contains("a@example.com") && raw.contains("b@example.com"));
        assert!(!raw.lines().any(|l| l.starts_with("second line")), "a line break in the title must not start a new header/body line");
        assert!(!raw.contains("X-Priority"));
        let hi = String::from_utf8_lossy(&build(&cfg(), &msg(5, false)).unwrap().formatted()).to_string();
        assert!(hi.contains("X-Priority") && !hi.contains("multipart/mixed"));
    }

    #[test]
    fn bad_addresses_are_refused_with_a_clear_message() {
        let c = EmailConfig { to: "not-an-address".into(), ..cfg() };
        assert!(build(&c, &msg(3, false)).unwrap_err().to_string().contains("not a valid email address"));
        let c = EmailConfig { username: String::new(), from: String::new(), ..cfg() };
        assert!(build(&c, &msg(3, false)).unwrap_err().to_string().contains("From"));
    }

    /// A throw-away SMTP server on localhost takes the message and we read
    /// back what arrived (no TLS: security "none").
    #[tokio::test]
    async fn delivers_through_a_real_smtp_conversation() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (sock, _) = l.accept().await.unwrap();
            let (r, mut w) = sock.into_split();
            let mut r = BufReader::new(r);
            w.write_all(b"220 test ESMTP\r\n").await.unwrap();
            let (mut got, mut data, mut line) = (String::new(), false, String::new());
            loop {
                line.clear();
                if r.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                if data {
                    if line == ".\r\n" {
                        data = false;
                        w.write_all(b"250 queued\r\n").await.unwrap();
                    } else {
                        got.push_str(&line);
                    }
                    continue;
                }
                let up = line.to_uppercase();
                let reply: &[u8] = if up.starts_with("EHLO") {
                    b"250 test\r\n"
                } else if up.starts_with("DATA") {
                    data = true;
                    b"354 go\r\n"
                } else if up.starts_with("QUIT") {
                    w.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    b"250 ok\r\n"
                };
                w.write_all(reply).await.unwrap();
            }
            got
        });
        let c = EmailConfig { host: "127.0.0.1".into(), port, security: "none".into(), username: String::new(), from: "me@example.com".into(), ..cfg() };
        send(&c, &msg(3, true)).await.unwrap();
        let got = server.await.unwrap();
        assert!(got.contains("Subject: Front Door: Person second line"), "{got}");
        assert!(got.contains("A person, 12 s") && got.contains("alert.jpg"), "{got}");
    }
}
