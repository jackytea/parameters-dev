use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub type MailFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

pub trait Mailer: Send + Sync {
    fn send(&self, mail: Mail) -> MailFuture;
}

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    pub fn new(url: &str, from: &str) -> Result<Self, String> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|_| "PARAMETERS_SMTP_URL is not a valid smtp:// or smtps:// URL".to_owned())?
            .build();
        let from = from
            .parse()
            .map_err(|_| "PARAMETERS_MAIL_FROM is not a valid address".to_owned())?;
        Ok(Self { transport, from })
    }
}

impl Mailer for SmtpMailer {
    fn send(&self, mail: Mail) -> MailFuture {
        let transport = self.transport.clone();
        let from = self.from.clone();
        Box::pin(async move {
            let to: Mailbox = mail
                .to
                .parse()
                .map_err(|_| format!("`{}` is not a valid email address", mail.to))?;
            let message = Message::builder()
                .from(from)
                .to(to)
                .subject(mail.subject)
                .body(mail.text)
                .map_err(|error| error.to_string())?;
            transport
                .send(message)
                .await
                .map(drop)
                .map_err(|error| format!("cannot send the email: {error}"))
        })
    }
}

#[derive(Clone, Default)]
pub struct MemoryMailer {
    sent: Arc<Mutex<Vec<Mail>>>,
}

impl MemoryMailer {
    #[must_use]
    pub fn sent(&self) -> Vec<Mail> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Mailer for MemoryMailer {
    fn send(&self, mail: Mail) -> MailFuture {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mail);
        Box::pin(async { Ok(()) })
    }
}
