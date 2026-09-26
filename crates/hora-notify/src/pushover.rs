//! Pushover notifier: POSTs a message to the Pushover API.

use async_trait::async_trait;
use reqwest::Client;

use crate::util::{
    alert_phrase, budget_burn_phrase, cert_expiry_phrase, domain_expiry_phrase, event_suffix,
    latency_suffix, send_retrying, title_and_body, topology_suffix, vantage_suffix,
};
use crate::{AlertSeverity, Event, Notifier};

const PUSHOVER_API: &str = "https://api.pushover.net/1/messages.json";

pub struct PushoverNotifier {
    client: Client,
    token: String,
    user: String,
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

impl PushoverNotifier {
    #[must_use]
    pub fn new(client: Client, token: String, user: String) -> Self {
        Self {
            client,
            token,
            user,
        }
    }

    fn message(event: Event<'_>) -> (String, i8) {
        match event {
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
                    1,
                )
            }
            Event::Degraded {
                monitor,
                latency_ms,
            } => (
                format!("DEGRADED: {monitor}{}", latency_suffix(latency_ms)),
                0,
            ),
            Event::Recovered { monitor } => (format!("\u{1F7E2} RECOVERED: {monitor}"), -1),
            Event::CertExpiring { monitor, days_left } => (
                format!("CERT: {monitor} {}", cert_expiry_phrase(days_left)),
                0,
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
                0,
            ),
            Event::ReleaseAvailable(release) => (release_text(&release), 0),
            Event::Digest { period, summary } => (format!("DIGEST ({period}):\n{summary}"), -1),
            Event::PeerLinkDegraded { peer, witness } => (
                format!("PEER: {peer} unreachable, but {witness} sees it up (partition)"),
                0,
            ),
            Event::CertChanged {
                monitor,
                old_fingerprint,
                new_fingerprint,
            } => (
                format!("CERT CHANGED: {monitor}\nold: {old_fingerprint}\nnew: {new_fingerprint}"),
                1,
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
                1,
            ),
            Event::Alert {
                monitor,
                severity,
                title,
                message,
            } => {
                // Pushover priority: -1 quiet, 0 normal, 1 high. We stop at 1 -
                // priority 2 (emergency) would require retry/expire parameters.
                let priority = match severity {
                    AlertSeverity::Info => -1,
                    AlertSeverity::Warning => 0,
                    AlertSeverity::Error | AlertSeverity::Critical => 1,
                };
                (alert_phrase(monitor, severity, title, message), priority)
            }
        }
    }
}

/// The JSON payload for a rendered message and priority: the message's
/// headline becomes the title when there is a body to go under it, so the
/// Pushover list shows what happened instead of a fixed app label.
fn payload_json(message: &str, priority: i8) -> serde_json::Value {
    let (title, body) = title_and_body(message);
    let mut payload = serde_json::json!({
        "message": body,
        "priority": priority,
    });
    if let Some(title) = title {
        payload["title"] = title.into();
    }
    payload
}

#[async_trait]
impl Notifier for PushoverNotifier {
    fn name(&self) -> &'static str {
        "pushover"
    }

    async fn notify(&self, event: Event<'_>) -> anyhow::Result<()> {
        let (message, priority) = Self::message(event);
        let build = || {
            let mut payload = payload_json(&message, priority);
            payload["token"] = self.token.as_str().into();
            payload["user"] = self.user.as_str().into();
            self.client.post(PUSHOVER_API).json(&payload)
        };
        // The user key is a quasi-secret too: it lets anyone message the user.
        send_retrying(
            build,
            "pushover",
            &[self.token.as_str(), self.user.as_str()],
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_the_message_headline() {
        let payload = payload_json("\u{1F534} DOWN: API\nboom", 1);
        assert_eq!(payload["title"], "\u{1F534} DOWN: API");
        assert_eq!(payload["message"], "boom");

        // A single-line message carries no title at all: it would only
        // duplicate the body.
        let payload = payload_json("\u{1F7E2} RECOVERED: API", -1);
        assert!(payload.get("title").is_none());
        assert_eq!(payload["message"], "\u{1F7E2} RECOVERED: API");
    }
}
