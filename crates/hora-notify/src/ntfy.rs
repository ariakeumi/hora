//! ntfy notifier: POSTs a plain-text message to an ntfy topic.

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::HeaderValue;

use crate::util::{send_retrying, title_and_message};
use crate::{AlertSeverity, Event, Notifier};

pub struct NtfyNotifier {
    client: Client,
    url: String,
    token: Option<String>,
}

impl NtfyNotifier {
    #[must_use]
    pub fn new(client: Client, url: String, token: Option<String>) -> Self {
        Self { client, url, token }
    }
}

/// The ntfy tag and priority (1 min - 5 max) for an event, mapped from its
/// nature or, for a pushed alert, its severity.
fn tags_and_priority(event: Event<'_>) -> (&'static str, u8) {
    match event {
        Event::Down { .. } => ("rotating_light", 4),
        Event::Degraded { .. } | Event::PeerLinkDegraded { .. } => ("warning", 3),
        Event::Recovered { .. } => ("white_check_mark", 2),
        Event::CertExpiring { .. } => ("lock", 3),
        Event::DomainExpiring { .. } => ("globe_with_meridians", 3),
        Event::ReleaseAvailable { .. } => ("package", 3),
        Event::Digest { .. } => ("bar_chart", 2),
        Event::CertChanged { .. } => ("lock", 4),
        Event::BudgetBurn { .. } => ("fire", 4),
        Event::Alert { severity, .. } => match severity {
            AlertSeverity::Info => ("information_source", 2),
            AlertSeverity::Warning => ("warning", 3),
            AlertSeverity::Error => ("rotating_light", 4),
            AlertSeverity::Critical => ("rotating_light", 5),
        },
    }
}

/// The `Title` header for a notification. Header values carry visible ASCII
/// plus obs-text bytes, so UTF-8 titles (a monitor name in any script)
/// travel; one with characters no header may carry (control characters)
/// returns `None` instead of a value reqwest would reject.
fn title_header(title: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(title).ok()
}

#[async_trait]
impl Notifier for NtfyNotifier {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    async fn notify(&self, event: Event<'_>) -> anyhow::Result<()> {
        let (title, message) = title_and_message(event);
        let (tags, priority) = tags_and_priority(event);
        let build = || {
            let mut req = self.client.post(&self.url).body(message.clone());
            if let Some(head) = title_header(&title) {
                req = req.header("Title", head);
            }
            req = req.header("Tags", tags);
            req = req.header("Priority", priority.to_string());
            if let Some(token) = &self.token {
                req = req.bearer_auth(token);
            }
            req
        };
        // The topic URL itself is the capability; the bearer token must not
        // surface either if the server echoes it in a rejection body.
        let mut secrets = vec![self.url.as_str()];
        if let Some(token) = &self.token {
            secrets.push(token.as_str());
        }
        send_retrying(build, "ntfy", &secrets).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_header_takes_utf8_but_not_control_characters() {
        assert!(title_header("Hora: API").is_some());
        assert!(title_header("Hora: 数据库").is_some());
        assert!(title_header("Hora: API\u{0}").is_none());
    }
}
