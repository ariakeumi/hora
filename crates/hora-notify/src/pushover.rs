//! Pushover notifier: POSTs a message to the Pushover API.

use async_trait::async_trait;
use reqwest::Client;

use crate::util::{send_retrying, title_and_message};
use crate::{AlertSeverity, Event, Notifier};

const PUSHOVER_API: &str = "https://api.pushover.net/1/messages.json";

pub struct PushoverNotifier {
    client: Client,
    token: String,
    user: String,
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

    /// Pushover priority: -1 quiet, 0 normal, 1 high. We stop at 1 - priority
    /// 2 (emergency) would require retry/expire parameters.
    fn priority(event: Event<'_>) -> i8 {
        match event {
            Event::Down { .. } | Event::CertChanged { .. } | Event::BudgetBurn { .. } => 1,
            Event::Degraded { .. }
            | Event::CertExpiring { .. }
            | Event::DomainExpiring { .. }
            | Event::ReleaseAvailable { .. }
            | Event::PeerLinkDegraded { .. } => 0,
            Event::Recovered { .. } | Event::Digest { .. } => -1,
            Event::Alert { severity, .. } => match severity {
                AlertSeverity::Info => -1,
                AlertSeverity::Warning => 0,
                AlertSeverity::Error | AlertSeverity::Critical => 1,
            },
        }
    }
}

#[async_trait]
impl Notifier for PushoverNotifier {
    fn name(&self) -> &'static str {
        "pushover"
    }

    async fn notify(&self, event: Event<'_>) -> anyhow::Result<()> {
        let (title, message) = title_and_message(event);
        let priority = Self::priority(event);
        let build = || {
            self.client.post(PUSHOVER_API).json(&serde_json::json!({
                "token": self.token,
                "user": self.user,
                "title": title,
                "message": message,
                "priority": priority,
            }))
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
    fn priorities_map_from_the_event() {
        let down = Event::Down {
            monitor: "API",
            error: None,
            cause: None,
            impacted: &[],
            vantage: None,
            event: None,
        };
        assert_eq!(PushoverNotifier::priority(down), 1);
        assert_eq!(
            PushoverNotifier::priority(Event::Recovered { monitor: "API" }),
            -1
        );
        // A pushed alert rides its producer-supplied severity.
        let alert = Event::Alert {
            monitor: "API",
            severity: AlertSeverity::Critical,
            title: "disk full",
            message: "",
        };
        assert_eq!(PushoverNotifier::priority(alert), 1);
    }
}
