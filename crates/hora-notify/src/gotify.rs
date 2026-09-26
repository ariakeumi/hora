//! Gotify notifier: POSTs a message to a Gotify server.

use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use crate::util::{
    alert_phrase, budget_burn_phrase, cert_expiry_phrase, domain_expiry_phrase, event_suffix,
    latency_suffix, send_retrying, title_and_body, topology_suffix, vantage_suffix,
};
use crate::{AlertSeverity, Event, Notifier};

pub struct GotifyNotifier {
    client: Client,
    url: String,
    token: String,
}

/// A release event as one text: what is out, what runs, where the notes are.
fn release_text(release: &crate::Release<'_>) -> String {
    format!(
        "RELEASE: {}: {}\n{}",
        release.monitor,
        crate::util::release_phrase(release),
        release.url
    )
}

impl GotifyNotifier {
    #[must_use]
    pub fn new(client: Client, url: String, token: String) -> Self {
        Self { client, url, token }
    }

    fn payload(event: Event<'_>) -> Payload {
        let (message, priority) = match event {
            Event::Down {
                monitor,
                error,
                cause,
                impacted,
                vantage,
                event,
            } => {
                let suffix = topology_suffix(cause, impacted);
                let vantage = vantage_suffix(vantage);
                let event = event_suffix(event);
                let detail = error.map_or_else(String::new, |e| format!("\n{e}"));
                (
                    format!("\u{1F534} DOWN: {monitor}{detail}{suffix}{vantage}{event}"),
                    8,
                )
            }
            Event::Degraded {
                monitor,
                latency_ms,
            } => (
                format!("DEGRADED: {monitor}{}", latency_suffix(latency_ms)),
                5,
            ),
            Event::Recovered { monitor } => (format!("\u{1F7E2} RECOVERED: {monitor}"), 2),
            Event::CertExpiring { monitor, days_left } => (
                format!("CERT: {monitor} {}", cert_expiry_phrase(days_left)),
                5,
            ),
            Event::DomainExpiring {
                monitor,
                domain,
                days_left,
            } => (
                format!(
                    "DOMAIN: {monitor} {}",
                    domain_expiry_phrase(domain, days_left)
                ),
                5,
            ),
            Event::ReleaseAvailable(release) => (release_text(&release), 3),
            Event::Digest { period, summary } => (format!("DIGEST ({period}):\n{summary}"), 2),
            Event::PeerLinkDegraded { peer, witness } => (
                format!("PEER: {peer} unreachable, but {witness} sees it up (partition)"),
                5,
            ),
            Event::CertChanged {
                monitor,
                old_fingerprint,
                new_fingerprint,
            } => (
                format!("CERT CHANGED: {monitor}\nold: {old_fingerprint}\nnew: {new_fingerprint}"),
                8,
            ),
            Event::BudgetBurn {
                monitor,
                burn_rate_x10,
                window,
                exhausted_in_secs,
            } => (
                format!(
                    "BUDGET: {monitor} {}",
                    budget_burn_phrase(burn_rate_x10, window, exhausted_in_secs)
                ),
                8,
            ),
            Event::Alert {
                monitor,
                severity,
                title,
                message,
            } => {
                // Gotify priority runs 0..10 (4-7 normal, 8+ high): map severity on.
                let priority = match severity {
                    AlertSeverity::Info => 2,
                    AlertSeverity::Warning => 5,
                    AlertSeverity::Error => 8,
                    AlertSeverity::Critical => 9,
                };
                (alert_phrase(monitor, severity, title, message), priority)
            }
        };
        // The message's headline becomes the title, so the notification list
        // shows what happened instead of a fixed app label.
        let (title, body) = title_and_body(&message);
        Payload {
            title: title.map(str::to_owned),
            message: body.to_owned(),
            priority,
        }
    }
}

#[derive(Serialize)]
struct Payload {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
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
    fn the_title_is_the_message_headline() {
        let down = GotifyNotifier::payload(Event::Down {
            monitor: "API",
            error: Some("boom"),
            cause: None,
            impacted: &[],
            vantage: None,
            event: None,
        });
        assert_eq!(down.title.as_deref(), Some("\u{1F534} DOWN: API"));
        assert_eq!(down.message, "boom");

        // A single-line message carries no title: it would only duplicate the
        // body, which the clients headline themselves.
        let recovered = GotifyNotifier::payload(Event::Recovered { monitor: "API" });
        assert_eq!(recovered.title, None);
        assert_eq!(recovered.message, "\u{1F7E2} RECOVERED: API");
    }
}
