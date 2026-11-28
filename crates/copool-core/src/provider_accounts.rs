//! An unofficial provider's accounts, whatever its kind, as cards of a pool: what an account keeps, the cards a
//! provider's accounts make, how a refresh's accounts are taken, and the
//! form every kind's provider is checked by.
use crate::{
    models::{AccountSummary, UsageSnapshot},
    pool::Pool,
    provider_registry::Kind,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    #[serde(rename = "accountID", alias = "accountId")]
    pub account_id: Option<String>,
    pub account_type: String,
    pub status: String,
    pub plan_type: Option<String>,
    pub usage: Option<UsageSnapshot>,
    pub usage_error: Option<String>,
    #[serde(default, rename = "providerID", alias = "providerId")]
    pub provider_id: Option<String>,
    /// The provider's Sub2api version, for an Antigravity or Grok account,
    /// when it predates the pool's weekly usage (`weekly_since`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outdated_version: Option<String>,
}
impl Account {
    pub fn display_email(&self) -> &str {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("--")
    }
    /// Show Sub2api's account name when the upstream email is unavailable.
    fn shown_email(&self) -> Option<String> {
        [self.email.as_deref(), Some(self.name.as_str())]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(str::to_owned)
    }
    /// Keeps what the account's last reading knew when its usage failed:
    /// its identity, plan and usage, with the error, as an imported
    /// account does.
    pub(crate) fn keep_last(&mut self, last: Option<Account>) {
        if self.usage.is_some() {
            return;
        }
        let Some(last) = last else {
            return;
        };
        self.email = self.email.take().or(last.email);
        self.account_id = self.account_id.take().or(last.account_id);
        self.usage = last.usage;
        if let (Some(plan), Some(usage)) = (&self.plan_type, &mut self.usage) {
            usage.plan_type = Some(plan.clone());
        }
        self.plan_type = self.plan_type.take().or(last.plan_type);
    }
    /// The account's card, by its id in the pool.
    pub fn summary(&self, id: String) -> AccountSummary {
        AccountSummary {
            display_status: "list".into(),
            workspace_status: if self.status.eq_ignore_ascii_case("active") {
                "active"
            } else {
                "deactivated"
            }
            .into(),
            id,
            label: self.shown_email().unwrap_or_else(|| "--".into()),
            email: self.shown_email(),
            account_id: self
                .account_id
                .clone()
                .unwrap_or_else(|| self.id.to_string()),
            plan_type: self.plan_type.clone(),
            team_name: None,
            // Its provider's kind names it (`Member::summaries`).
            source_tag: None,
            added_at: self.id,
            updated_at: self.usage.as_ref().map(|u| u.fetched_at).unwrap_or(0),
            usage: self.usage.clone().map(UsageSnapshot::with_matching_windows),
            usage_error: self.usage_error.clone(),
            is_current: false,
            awaiting_first_request: false,
            token_expires_at: None,
            renewal: None,
            next_refresh_at: None,
            refresh: None,
            available: self.status.eq_ignore_ascii_case("active"),
            proxy_url: String::new(),
            outdated_sub2api_version: self.outdated_version.clone(),
            provider_kind: None,
            tags: Default::default(),
        }
    }
}
/// A card's id: its pool, provider and account. A provider's id is unique
/// whatever its kind, so every kind's cards share one prefix, the one
/// Sub2api's cards always had: saved orders and refresh switches keep
/// naming them.
pub(crate) fn card_id(pool: Pool, provider: &str, account: i64) -> String {
    format!("{}{}-{account}", card_prefix(pool), provider.trim())
}
fn card_prefix(pool: Pool) -> &'static str {
    match pool {
        Pool::Codex => "sub2api-",
        Pool::Claude => "claude-sub2api-",
        Pool::Antigravity => "antigravity-sub2api-",
        Pool::Grok => "grok-sub2api-",
    }
}
/// Whether a card of the pool is an unofficial provider's account's.
pub(crate) fn is_card(pool: Pool, id: &str) -> bool {
    id.starts_with(card_prefix(pool))
}
/// The provider of a provider card of the pool, as its id names it.
pub(crate) fn card_provider(pool: Pool, id: &str) -> Option<&str> {
    let (provider, account) = id.strip_prefix(card_prefix(pool))?.rsplit_once('-')?;
    account.parse::<i64>().ok().map(|_| provider)
}
/// Where the keyring keeps a provider's secret: Sub2api's administrator
/// password.
pub(crate) fn password_secret(pool: Pool, id: &str) -> String {
    match pool {
        Pool::Codex => format!("codex-sub2api-password:{id}"),
        Pool::Claude => format!("claude-sub2api-password:{id}"),
        Pool::Antigravity => format!("antigravity-sub2api-password:{id}"),
        Pool::Grok => format!("grok-sub2api-password:{id}"),
    }
}
/// A provider's accounts in a pool: those imported, as last fetched.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    /// What the provider is.
    pub(crate) kind: Kind,
    /// The provider as its cards' ids name it.
    pub(crate) key: String,
    /// The provider as shown, and in its errors.
    pub(crate) name: String,
    /// The proxy of every request for the provider and its accounts.
    pub(crate) proxy_url: String,
    pub(crate) imported: Vec<i64>,
    /// Accounts whose cards the user removed, until a sync.
    pub(crate) removed: Vec<i64>,
    pub(crate) cached: Vec<Account>,
}
impl Member {
    pub(crate) fn card_id(&self, pool: Pool, account: i64) -> String {
        card_id(pool, &self.key, account)
    }
    /// The provider's cards; current while its pool uses the provider.
    pub(crate) fn summaries(&self, pool: Pool, current: bool) -> Vec<AccountSummary> {
        self.cached
            .iter()
            .filter(|a| a.status.eq_ignore_ascii_case("active"))
            .map(|a| {
                let mut summary = a.summary(self.card_id(pool, a.id));
                summary.source_tag = Some(self.kind.tag().into());
                summary.provider_kind = Some(self.kind.name().into());
                summary.is_current = current;
                summary.proxy_url = self.proxy_url.clone();
                summary
            })
            .collect()
    }
    /// Takes a refresh's accounts: a sync's are the provider's accounts from
    /// then on. A refresh of every card adds those it discovers, but none
    /// whose card the user removed, which only a sync restores; an account
    /// it asked about (`ids`, every one without) and no longer lists loses
    /// its card. Keep the imported ids so a provider with no active accounts
    /// still refreshes and can show them again when reactivated.
    pub(crate) fn adopt(
        &mut self,
        fetched: Vec<Account>,
        ids: Option<&[i64]>,
        sync: bool,
    ) -> usize {
        if sync {
            self.imported = fetched.iter().map(|a| a.id).collect();
            self.removed.clear();
            self.cached = fetched;
            return self.cached.len();
        }
        // Respect a removal made while the request ran.
        if self.imported.is_empty() {
            return 0;
        }
        let missing = |a: &Account| {
            ids.is_none_or(|ids| ids.contains(&a.id)) && !fetched.iter().any(|f| f.id == a.id)
        };
        self.cached.retain(|a| !missing(a));
        for account in fetched {
            if !self.imported.contains(&account.id) {
                // Refreshing some cards adds none: one removed while the
                // request ran stays removed.
                if ids.is_some() || self.removed.contains(&account.id) {
                    continue;
                }
                self.imported.push(account.id);
            }
            match self.cached.iter_mut().find(|a| a.id == account.id) {
                Some(cached) => *cached = account,
                None => self.cached.push(account),
            }
        }
        self.cached.len()
    }
}
/// Checks a provider form: its name unique in the pool, its sign-in
/// complete (a saved provider keeps its secret), and its proxy and URLs.
pub(crate) fn validate_form(
    name: &str,
    duplicate: bool,
    username: &str,
    password: &str,
    saved: bool,
    proxy: &str,
    urls: &[&str],
) -> Result<(), String> {
    if duplicate {
        return Err("error.sub2api.duplicate_provider".into());
    }
    if name.is_empty() || username.is_empty() || (password.is_empty() && !saved) {
        return Err("error.sub2api.configuration_incomplete".into());
    }
    crate::utils::validate_account_proxy(proxy)?;
    for url in urls.iter().filter(|url| !url.is_empty()) {
        let url = reqwest::Url::parse(url).map_err(|_| "error.sub2api.invalid_base_url")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("error.sub2api.invalid_base_url".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Account, Member};
    use crate::{provider_registry::Kind, Pool};

    fn account(id: i64, used: f64) -> Account {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": format!("account{id}"), "accountType": "oauth",
            "status": "active", "providerID": "p",
            "usage": {"fetchedAt": 0, "fiveHour": {"usedPercent": used, "windowSeconds": 18000}},
        }))
        .unwrap()
    }

    #[test]
    fn refreshing_one_card_does_not_restore_a_card_removed_meanwhile() {
        // Card 42 was removed while its refresh ran; 43 remains.
        let mut member = Member {
            kind: Kind::Sub2api,
            key: "p".into(),
            name: "p".into(),
            proxy_url: String::new(),
            imported: vec![43],
            removed: vec![],
            cached: vec![account(43, 10.)],
        };
        member.adopt(vec![account(42, 20.)], Some(&[42]), false);
        assert_eq!(member.imported, [43]);
        assert_eq!(member.cached.iter().map(|a| a.id).collect::<Vec<_>>(), [43]);
        // A refresh of every card still adds the accounts it discovers.
        member.adopt(vec![account(43, 30.), account(44, 0.)], None, false);
        assert_eq!(member.imported, [43, 44]);
    }

    #[test]
    fn only_a_sync_restores_a_card_the_user_removed() {
        // Of two imported accounts, the user removed 42's card; 43 remains.
        let mut member = Member {
            kind: Kind::Sub2api,
            key: "p".into(),
            name: "p".into(),
            proxy_url: String::new(),
            imported: vec![43],
            removed: vec![42],
            cached: vec![account(43, 10.)],
        };
        member.adopt(vec![account(42, 20.), account(43, 30.)], None, false);
        assert_eq!(member.imported, [43]);
        assert_eq!(member.cached.iter().map(|a| a.id).collect::<Vec<_>>(), [43]);
        member.adopt(vec![account(42, 20.), account(43, 30.)], None, true);
        assert_eq!(member.imported, [42, 43]);
        assert!(member.removed.is_empty());
    }

    #[test]
    fn inactive_and_missing_accounts_lose_their_cards_in_every_pool() {
        let mut member = Member {
            kind: Kind::Sub2api,
            key: "p".into(),
            name: "p".into(),
            proxy_url: String::new(),
            imported: vec![42, 43],
            removed: vec![],
            cached: vec![account(42, 10.), account(43, 10.)],
        };
        // Old cached inactive cards are hidden even before a refresh succeeds.
        member.cached[0].status = "disabled".into();
        for pool in Pool::ALL {
            assert_eq!(member.summaries(pool, false).len(), 1);
        }
        // Refreshing one card does not remove an unrelated card.
        member.adopt(vec![], Some(&[42]), false);
        assert_eq!(member.cached.iter().map(|a| a.id).collect::<Vec<_>>(), [43]);
        member.adopt(vec![], None, false);
        assert_eq!(member.imported, [42, 43]);
        for pool in Pool::ALL {
            assert!(member.summaries(pool, false).is_empty());
        }
        member.adopt(vec![account(42, 30.)], None, false);
        assert_eq!(member.cached.len(), 1);
        assert_eq!(member.cached[0].id, 42);
    }
}
