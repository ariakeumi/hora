//! Gotify notifier: POSTs a message to a Gotify server.

use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use crate::util::{send_retrying, title_and_message};
use crate::{AlertSeverity, Event, Notifier};

pub struct GotifyNotifier {
    client: Client,
    url: String,
    token: String,
}

impl GotifyNotifier {
    #[must_use]
    pub fn new(client: Client, url: String, token: String) -> Self {
        Self { client, url, token }
    }

    /// The shared `Hora: <monitor>` title/body template, plus Gotify's
    /// priority: 0..10 (4-7 normal, 8+ high), mapped from the event's nature
    /// or, for a pushed alert, its severity.
    fn payload(event: Event<'_>) -> Payload {
        let (title, message) = title_and_message(event);
        let priority = match event {
            Event::Down { .. } | Event::CertChanged { .. } | Event::BudgetBurn { .. } => 8,
            Event::Degraded { .. }
            | Event::CertExpiring { .. }
            | Event::DomainExpiring { .. }
            | Event::PeerLinkDegraded { .. } => 5,
            Event::Recovered { .. } | Event::Digest { .. } => 2,
            Event::ReleaseAvailable { .. } => 3,
            Event::Alert { severity, .. } => match severity {
                AlertSeverity::Info => 2,
                AlertSeverity::Warning => 5,
                AlertSeverity::Error => 8,
                AlertSeverity::Critical => 9,
            },
        };
        Payload {
            title,
            message,
            priority,
        }
    }
}

#[derive(Serialize)]
struct Payload {
    title: String,
    message: String,
    priority: u8,
}

#[async_trait]
impl Notifier for GotifyNotifier {
    fn name(&self) -> &'static str {
        "gotify"
    }

    async fn notify(&self, event: Event<'_>) -> anyhow::Result<()> {
        let payload = Self::payload(event);
        // The token travels as a header, not in the URL: query strings end up
        // in proxy and server access logs.
        let url = format!("{}/message", self.url.trim_end_matches('/'));
        let build = || {
            self.client
                .post(&url)
                .header("X-Gotify-Key", &self.token)
                .json(&payload)
        };
        send_retrying(build, "gotify", &[self.token.as_str()]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_monitor_and_the_body_carries_the_event() {
        let down = GotifyNotifier::payload(Event::Down {
            monitor: "API",
            error: Some("boom"),
            cause: None,
            impacted: &[],
            vantage: None,
            event: None,
        });
        assert_eq!(down.title, "Hora: API");
        assert_eq!(down.message, "\u{1F534} Down: API\nboom");
        assert_eq!(down.priority, 8);

        let recovered = GotifyNotifier::payload(Event::Recovered { monitor: "API" });
        assert_eq!(recovered.title, "Hora: API");
        assert_eq!(recovered.message, "\u{1F7E2} Recovered: API");
        assert_eq!(recovered.priority, 2);
    }
}
