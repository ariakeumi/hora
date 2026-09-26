//! ntfy notifier: POSTs a plain-text message to an ntfy topic.

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::HeaderValue;

use crate::util::{
    alert_phrase, budget_burn_phrase, cert_expiry_phrase, domain_expiry_phrase, event_suffix,
    latency_suffix, send_retrying, title_and_body, topology_suffix, vantage_suffix,
};
use crate::{AlertSeverity, Event, Notifier};

pub struct NtfyNotifier {
    client: Client,
    url: String,
    token: Option<String>,
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

impl NtfyNotifier {
    #[must_use]
    pub fn new(client: Client, url: String, token: Option<String>) -> Self {
        Self { client, url, token }
    }

    fn message(event: Event<'_>) -> (String, &'static str, u8) {
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
                    "rotating_light",
                    4,
                )
            }
            Event::Degraded {
                monitor,
                latency_ms,
            } => (
                format!("DEGRADED: {monitor}{}", latency_suffix(latency_ms)),
                "warning",
                3,
            ),
            Event::Recovered { monitor } => (
                format!("\u{1F7E2} RECOVERED: {monitor}"),
                "white_check_mark",
                2,
            ),
            Event::CertExpiring { monitor, days_left } => (
                format!("CERT: {monitor} {}", cert_expiry_phrase(days_left)),
                "lock",
                3,
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
                "globe_with_meridians",
                3,
            ),
            Event::ReleaseAvailable(release) => (release_text(&release), "package", 3),
            Event::Digest { period, summary } => {
                (format!("DIGEST ({period}):\n{summary}"), "bar_chart", 2)
            }
            Event::PeerLinkDegraded { peer, witness } => (
                format!("PEER: {peer} unreachable, but {witness} sees it up (partition)"),
                "warning",
                3,
            ),
            Event::CertChanged {
                monitor,
                old_fingerprint,
                new_fingerprint,
            } => (
                format!("CERT CHANGED: {monitor}\nold: {old_fingerprint}\nnew: {new_fingerprint}"),
                "lock",
                4,
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
                "fire",
                4,
            ),
            Event::Alert {
                monitor,
                severity,
                title,
                message,
            } => {
                // ntfy priority runs 1 (min) to 5 (max): map the severity onto it.
                let (tag, priority) = match severity {
                    AlertSeverity::Info => ("information_source", 2),
                    AlertSeverity::Warning => ("warning", 3),
                    AlertSeverity::Error => ("rotating_light", 4),
                    AlertSeverity::Critical => ("rotating_light", 5),
                };
                (
                    alert_phrase(monitor, severity, title, message),
                    tag,
                    priority,
                )
            }
        }
    }
}

/// The `Title` header and the body for a rendered message: the headline (first
/// line, e.g. `DOWN: API`) becomes the title and the rest the body, so the
/// notification list shows what happened instead of a fixed app label. Header
/// values carry visible ASCII plus obs-text bytes, so UTF-8 headlines (a
/// monitor name in any script) travel as-is; a headline with characters no
/// header may carry (control characters) keeps the message whole and no
/// `Title` header is sent - one that reqwest cannot build would panic.
fn title_header_and_body(message: &str) -> (Option<String>, String) {
    let (title, body) = title_and_body(message);
    let title = title.filter(|t| HeaderValue::from_str(t).is_ok());
    // Without a header-encodable headline the message stays whole.
    let body = if title.is_some() {
        body.to_owned()
    } else {
        message.to_owned()
    };
    (title.map(str::to_owned), body)
}

#[async_trait]
impl Notifier for NtfyNotifier {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    async fn notify(&self, event: Event<'_>) -> anyhow::Result<()> {
        let (message, tags, priority) = Self::message(event);
        let (title, body) = title_header_and_body(&message);
        let build = || {
            let mut req = self.client.post(&self.url).body(body.clone());
            if let Some(title) = &title {
                req = req.header("Title", title);
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
    fn the_title_header_is_the_message_headline() {
        let (title, body) = title_header_and_body("\u{1F534} DOWN: API\nboom");
        assert_eq!(title.as_deref(), Some("\u{1F534} DOWN: API"));
        assert_eq!(body, "boom");

        // A single-line message carries no title at all: it would only
        // duplicate the body.
        let (title, body) = title_header_and_body("\u{1F7E2} RECOVERED: API");
        assert_eq!(title, None);
        assert_eq!(body, "\u{1F7E2} RECOVERED: API");

        // A non-ASCII headline (a monitor name in any script) still travels:
        // header values carry obs-text bytes, which ntfy reads as UTF-8.
        let (title, body) = title_header_and_body("DOWN: 数据库\nboom");
        assert_eq!(title.as_deref(), Some("DOWN: 数据库"));
        assert_eq!(body, "boom");

        // A headline with characters no header may carry (e.g. a control
        // character) falls back to the whole message and no title.
        let (title, body) = title_header_and_body("DOWN: API\u{0}\nboom");
        assert_eq!(title, None);
        assert_eq!(body, "DOWN: API\u{0}\nboom");
    }
}
