//! The account pools, Codex, Claude Code and Antigravity, behind one set of
//! operations.
//! Each pool keeps only what is its own: where its logins are stored and
//! which login its tool uses, how a login is renewed, and how usage is asked
//! for. Everything done with the cards (editing one, choosing the best,
//! switching automatically, renewing at once, saying hi, refreshing by hand
//! and in background rounds) is implemented here once, for both pools.
use crate::{
    application::{eligible, Application, SmartSwitchOutcome, SwitchExecution},
    models::{AccountSummary, Renewal, StoredAccount},
    refresh_policy::{self, Schedule},
    sub2api,
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde_json::Value;
use std::time::Duration;

type Result<T> = std::result::Result<T, String>;

/// The largest account document an import accepts.
const IMPORT_LIMIT: u64 = 16 * 1024 * 1024;

pub use crate::models::Pool;

/// A login's hi: said, or refused as expired with this error.
pub(crate) enum Hello {
    Said,
    Expired(String),
}

/// A pool account, as the operations both pools share edit it.
pub(crate) trait PoolAccount {
    fn id(&self) -> &str;
    fn set_renewal(&mut self, renewal: Renewal);
    fn set_proxy(&mut self, proxy: String);
    /// Lets a new account contact upstream; returns whether it waited.
    fn release(&mut self) -> bool;
}
impl PoolAccount for StoredAccount {
    fn id(&self) -> &str {
        &self.id
    }
    fn set_renewal(&mut self, renewal: Renewal) {
        StoredAccount::set_renewal(self, renewal);
    }
    fn set_proxy(&mut self, proxy: String) {
        self.proxy_url = proxy;
    }
    fn release(&mut self) -> bool {
        self.extra
            .remove(crate::models::FIRST_REQUEST_PENDING)
            .is_some()
    }
}

/// One pool's background refresh rounds.
#[derive(Default)]
pub(crate) struct Rounds {
    /// The next round; none before the first starts.
    schedule: std::sync::Mutex<Option<Schedule>>,
    /// Its wait, started over by refreshing the whole pool.
    deferred: tokio::sync::Notify,
    /// Rounds in a row whose usage did not change.
    unchanged: std::sync::Mutex<u8>,
}

impl Application {
    fn rounds(&self, pool: Pool) -> &Rounds {
        &self.rounds[pool as usize]
    }
    /// The pool's cards, following the login its tool uses.
    pub async fn accounts(&self, pool: Pool) -> Result<Vec<AccountSummary>> {
        match pool {
            Pool::Codex => self.codex_accounts().await,
            Pool::Claude => self.claude_accounts().await,
            Pool::Antigravity => self.antigravity_accounts().await,
        }
    }
    /// Lets a user action ask for the Keychain access background work does
    /// not, as after a build changed Copool's signature.
    pub async fn authorize(&self, pool: Pool) -> Result<()> {
        match pool {
            Pool::Codex => self.authorize_keychain().await,
            Pool::Claude => self.claude_authorize().await,
            // Antigravity's login is asked for only to import or switch.
            Pool::Antigravity => Ok(()),
        }
    }
    /// Imports the login the pool's tool uses; returns how many accounts.
    pub async fn import_current(&self, pool: Pool) -> Result<usize> {
        match pool {
            Pool::Codex => self.codex_import_current().await,
            Pool::Claude => self.claude_import_current().await.map(|_| 1),
            Pool::Antigravity => self.antigravity_import_current().await,
        }
    }
    /// Imports exported logins, the whole document validated before any is
    /// stored; returns how many.
    pub async fn import_json(&self, pool: Pool, content: &str) -> Result<usize> {
        if content.len() as u64 > IMPORT_LIMIT {
            return Err("Import exceeds 16 MiB".into());
        }
        match pool {
            Pool::Codex => self.codex_import_json(content).await,
            Pool::Claude => self.claude_import_json(content).await,
            Pool::Antigravity => self.antigravity_import_json(content).await,
        }
    }
    /// Imports an exported login file.
    pub async fn import_file(&self, pool: Pool, path: &std::path::Path) -> Result<usize> {
        let path = path.to_owned();
        // Off the runtime, and no more than the limit, whatever the file
        // grows to meanwhile; a pipe or device is no login file.
        let content = tokio::task::spawn_blocking(move || -> Result<String> {
            use std::io::Read;
            if !std::fs::metadata(&path)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("Not a file".into());
            }
            let mut content = String::new();
            std::fs::File::open(&path)
                .and_then(|file| file.take(IMPORT_LIMIT + 1).read_to_string(&mut content))
                .map_err(|e| e.to_string())?;
            if content.len() as u64 > IMPORT_LIMIT {
                return Err("File exceeds 16 MiB".into());
            }
            Ok(content)
        })
        .await
        .map_err(|e| e.to_string())??;
        self.import_json(pool, &content).await
    }
    /// Stores a browser sign-in's login without switching to it.
    pub async fn import_sign_in(
        &self,
        pool: Pool,
        document: Value,
        workspace: Option<&str>,
    ) -> Result<usize> {
        match pool {
            Pool::Codex => {
                let consent = document
                    .get("consentWorkspaces")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.import_authorized(document, workspace, &consent).await
            }
            Pool::Claude => self.claude_import_login(document).await.map(|_| 1),
            Pool::Antigravity => self.antigravity_import_login(document).await,
        }
    }
    /// Removes a card; a Sub2api card leaves its provider until a sync.
    pub async fn delete_account(&self, pool: Pool, id: &str) -> Result<()> {
        if sub2api::is_card(pool, id) {
            return self.delete_card(pool, id).await;
        }
        match pool {
            Pool::Codex => self.codex_delete_account(id).await,
            Pool::Claude => self.claude_delete_account(id).await,
            Pool::Antigravity => self.antigravity_delete_account(id).await,
        }
    }
    /// A card's proxy: blank for the shared account proxy, "direct://" for
    /// none. A Sub2api card's is its provider's.
    pub async fn save_account_proxy(&self, pool: Pool, id: &str, value: &str) -> Result<()> {
        crate::utils::validate_account_proxy(value)?;
        let value = value.trim().to_owned();
        if sub2api::is_card(pool, id) {
            return self.set_card_proxy(pool, id, value).await;
        }
        self.edit_account(pool, id, |account| account.set_proxy(value))
            .await
    }
    /// Sets whether Copool renews an account's tokens: "never", "once" (then
    /// never) or "always".
    pub async fn set_renewal(&self, pool: Pool, id: &str, renewal: &str) -> Result<()> {
        let renewal = Renewal::parse(renewal)?;
        self.edit_account(pool, id, |account| account.set_renewal(renewal))
            .await
    }
    /// A manual refresh lets new accounts contact upstream: one account, or
    /// all of them without an id.
    pub async fn release_new_accounts(&self, pool: Pool, id: Option<&str>) -> Result<()> {
        self.edit_accounts(pool, |accounts| {
            let mut changed = false;
            for account in accounts.iter_mut() {
                if id.is_none_or(|id| account.id() == id) {
                    changed |= account.release();
                }
            }
            Ok(changed)
        })
        .await
    }
    async fn edit_account(
        &self,
        pool: Pool,
        id: &str,
        edit: impl FnOnce(&mut dyn PoolAccount) + Send,
    ) -> Result<()> {
        self.edit_accounts(pool, |accounts| {
            let account = accounts
                .iter_mut()
                .find(|a| a.id() == id)
                .ok_or("Account not found")?;
            edit(&mut **account);
            Ok(true)
        })
        .await
    }
    /// Edits the pool's stored accounts, saved when `edit` reports a change.
    async fn edit_accounts(
        &self,
        pool: Pool,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool> + Send,
    ) -> Result<()> {
        match pool {
            Pool::Codex => self.codex_edit_accounts(edit).await,
            Pool::Claude => self.claude_edit_accounts(edit).await,
            Pool::Antigravity => self.antigravity_edit_accounts(edit).await,
        }
    }
    /// Gives the pool's tool this card's login, or its provider.
    pub async fn switch_account(&self, pool: Pool, id: &str) -> Result<SwitchExecution> {
        self.switch_with_access(pool, id, KeychainAccess::Interactive)
            .await
    }
    async fn switch_with_access(
        &self,
        pool: Pool,
        id: &str,
        access: KeychainAccess,
    ) -> Result<SwitchExecution> {
        match pool {
            Pool::Codex => self.codex_switch_account(id).await,
            Pool::Claude => self.claude_switch_account(id, access).await,
            Pool::Antigravity => self.antigravity_switch_account(id, access).await,
        }
    }
    /// As Swift's AccountsPageModel.smartSwitch: the account with the most
    /// remaining quota, and no switch when that is already the current one.
    pub async fn smart_switch(&self, pool: Pool) -> Result<SmartSwitchOutcome> {
        let accounts = self.accounts(pool).await?;
        let score = crate::ranking::remaining_score;
        // Swift's max(by:) keeps the first of equal scores, so a tie with the
        // current account (listed first) is no reason to switch.
        let best =
            accounts
                .iter()
                .filter(|a| a.available)
                .fold(None::<&AccountSummary>, |best, a| match best {
                    Some(b) if score(a) <= score(b) => Some(b),
                    _ => Some(a),
                });
        let Some(best) = best else {
            return Ok(SmartSwitchOutcome::NoTarget);
        };
        if best.is_current {
            return Ok(SmartSwitchOutcome::AlreadyBest);
        }
        let execution = self.switch_account(pool, &best.id).await?;
        Ok(SmartSwitchOutcome::Switched {
            label: best.label.clone(),
            execution,
        })
    }
    /// Switches away from the current card once it runs low, when the
    /// preference allows, to the best fresh one; returns its id.
    pub async fn auto_switch_if_needed(&self, pool: Pool) -> Result<Option<String>> {
        if !self.preferences()?.auto_switch {
            return Ok(None);
        }
        let accounts = self.accounts(pool).await?;
        // A Sub2api provider's accounts, on its one card, are current
        // together; any fresh one will do.
        if !accounts.iter().any(|a| {
            a.is_current
                && a.usage_error.is_none()
                && a.usage
                    .as_ref()
                    .is_some_and(|u| (0..=900).contains(&(now_unix_seconds() - u.fetched_at)))
        }) {
            return Ok(None);
        }
        let candidates: Vec<_> = accounts
            .into_iter()
            .filter(|a| a.is_current || eligible(a))
            .collect();
        if let Some(target) = crate::ranking::auto_switch_target(&candidates) {
            self.switch_with_access(pool, &target.id, KeychainAccess::Background)
                .await?;
            return Ok(Some(target.id.clone()));
        }
        Ok(None)
    }
    /// Refreshes one card: an account's usage, its login renewed as its
    /// renewal allows, or a Sub2api card through its provider.
    pub async fn refresh_usage(&self, pool: Pool, id: &str) -> Result<()> {
        if sub2api::is_card(pool, id) {
            return self.refresh_card(pool, id).await;
        }
        self.refresh_login(pool, id, None).await
    }
    /// With `renew_now`, only the tokens are renewed, even when unexpired or
    /// renewal is off; usage is not asked for, nor shown as refreshing, and
    /// whether the pool kept the renewal is reported.
    async fn refresh_login(
        &self,
        pool: Pool,
        id: &str,
        renew_now: Option<&mut bool>,
    ) -> Result<()> {
        match pool {
            Pool::Codex => self.codex_refresh_account(id, true, renew_now).await,
            Pool::Claude => self.claude_refresh_usage_scoped(id, renew_now).await,
            Pool::Antigravity => self.antigravity_refresh(id, renew_now).await,
        }
    }
    /// Renews an account's tokens at once, whatever its renewal choice, which
    /// stays as it was. Usage is not asked for, unless the account had
    /// failed: its token expired or its usage refused; it then refreshes once
    /// renewed, and a usage error shows on the card.
    pub async fn renew_now(&self, pool: Pool, id: &str) -> Result<()> {
        if sub2api::is_card(pool, id) {
            return Err("Token renewal is not available for this account".into());
        }
        let failed = match pool {
            Pool::Codex => self.codex_login_failed(id).await?,
            Pool::Claude => self.claude_login_failed(id).await?,
            Pool::Antigravity => self.antigravity_login_failed(id).await?,
        };
        self.release_new_accounts(pool, Some(id)).await?;
        let mut renewed = false;
        let result = self.refresh_login(pool, id, Some(&mut renewed)).await;
        if !renewed {
            result?;
            return Err(match pool {
                Pool::Codex | Pool::Antigravity => "The token was not renewed",
                // Claude Code renewed the login meanwhile.
                Pool::Claude => crate::claude::CREDENTIALS_CHANGED,
            }
            .into());
        }
        if failed {
            let _ = self.refresh_usage(pool, id).await;
        }
        Ok(())
    }
    /// Says hi with a card's login, without switching to it, so that its
    /// five-hour window starts now; the usage reported shows it only later.
    /// A login refused as expired is renewed by a refresh, as its renewal
    /// allows, and says hi once more. A Sub2api card says hi through Sub2api.
    pub async fn say_hi(&self, pool: Pool, id: &str) -> Result<()> {
        let _greeting = self.begin_greeting(pool, id)?;
        if sub2api::is_card(pool, id) {
            return self.say_hi_through_provider(pool, id).await;
        }
        let mut renewed = false;
        loop {
            let hello = match pool {
                Pool::Codex => self.codex_hello(id).await?,
                Pool::Claude => self.claude_hello(id).await?,
                Pool::Antigravity => self.antigravity_hello(id).await?,
            };
            match hello {
                Hello::Said => return Ok(()),
                Hello::Expired(_) if !renewed => {
                    renewed = true;
                    self.refresh_usage(pool, id).await?;
                }
                Hello::Expired(error) => return Err(error),
            }
        }
    }
    /// A refresh the user asked for: one card, or the whole pool, which then
    /// stands in for its next background round. It may ask for the Keychain
    /// access background work does not, and lets new accounts contact
    /// upstream.
    pub async fn refresh_now(&self, pool: Pool, id: Option<&str>) -> Result<()> {
        let _ = self.authorize(pool).await;
        self.release_new_accounts(pool, id).await?;
        match id {
            Some(id) => self.refresh_usage(pool, id).await,
            None => {
                let result = self.refresh_all(pool).await;
                self.defer_background_round(pool);
                result
            }
        }
    }
    /// Every account and every Sub2api provider of the pool.
    pub async fn refresh_all(&self, pool: Pool) -> Result<()> {
        self.refresh_pool(pool, None, None).await
    }
    /// Refreshes the accounts by `ids` and the providers by `providers`
    /// (their keys), all of either without, then switches automatically as
    /// needed. They refresh concurrently: one slow or unreachable endpoint
    /// must not hold back the rest of the pool.
    pub(crate) async fn refresh_pool(
        &self,
        pool: Pool,
        ids: Option<&[String]>,
        providers: Option<&[String]>,
    ) -> Result<()> {
        let accounts = self.accounts(pool).await?;
        let refreshes = accounts
            .iter()
            .filter(|a| !sub2api::is_card(pool, &a.id) && ids.is_none_or(|ids| ids.contains(&a.id)))
            .map(|account| async move {
                self.refresh_usage(pool, &account.id)
                    .await
                    .map_err(|error| format!("{}: {error}", account.label))
            });
        let (accounts, providers) = tokio::join!(
            futures_util::future::join_all(refreshes),
            self.refresh_providers(pool, providers)
        );
        let errors: Vec<String> = accounts
            .into_iter()
            .filter_map(Result::err)
            .chain(providers)
            .collect();
        self.auto_switch_if_needed(pool).await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    /// A background round's refresh: the card in use and the provider in
    /// use, the whole pool and every provider on its full rounds. Claude and
    /// Antigravity ask for no account asked within the interval (by a manual
    /// refresh, say).
    pub async fn refresh_recurring(&self, pool: Pool, tick: u64) -> Result<()> {
        let accounts = self.accounts(pool).await?;
        let full_every = self.pool_refresh(pool).full_every;
        match pool {
            Pool::Codex => {
                let ids = refresh_policy::recurring_ids(tick, full_every, &accounts);
                let current = crate::codex_config::resolve_path(&self.codex_config_path()).id;
                let providers = ids.is_some().then(|| vec![current]);
                self.refresh_pool(pool, ids.as_deref(), providers.as_deref())
                    .await
            }
            Pool::Claude | Pool::Antigravity => {
                let now = now_unix_seconds();
                let interval = self.round_interval(pool).as_secs() as i64;
                let full = refresh_policy::is_full_round(tick, full_every);
                let ids: Vec<String> = accounts
                    .iter()
                    .filter(|a| {
                        (full || a.is_current)
                            && !sub2api::is_card(pool, &a.id)
                            && now - a.updated_at >= interval
                    })
                    .map(|a| a.id.clone())
                    .collect();
                // Between full rounds, only a provider in use.
                let providers: Option<Vec<String>> = (!full).then(|| {
                    accounts
                        .iter()
                        .filter(|a| a.is_current)
                        .filter_map(|a| sub2api::card_provider(pool, &a.id))
                        .map(str::to_owned)
                        .collect()
                });
                self.refresh_pool(pool, Some(&ids), providers.as_deref())
                    .await
            }
        }
    }
    pub async fn background(self: std::sync::Arc<Self>) {
        if let Ok(preferences) = self.preferences() {
            for (pool, auto) in [
                (Pool::Codex, preferences.auto_start_proxy),
                (Pool::Claude, preferences.claude_auto_start_proxy),
                (Pool::Antigravity, preferences.antigravity_auto_start_proxy),
            ] {
                if auto {
                    let _ = self.start_proxy(pool).await;
                }
            }
        }
        // Independent loops keep a slow provider from delaying the other pool.
        tokio::join!(
            self.background_pool(Pool::Codex),
            self.background_pool(Pool::Claude),
            self.background_pool(Pool::Antigravity),
            self.clone().background_greetings()
        );
    }
    async fn background_pool(&self, pool: Pool) {
        let mut tick = 0u64;
        loop {
            let mut wait = self.background_round(tick, pool).await;
            // Refreshing the whole pool starts the wait over.
            while tokio::select! {
                _ = tokio::time::sleep(wait) => false,
                _ = self.rounds(pool).deferred.notified() => true,
            } {
                wait = self.round_interval(pool);
            }
            tick = tick.wrapping_add(1);
        }
    }
    /// The pool's background refresh, as set for all its cards.
    pub(crate) fn pool_refresh(&self, pool: Pool) -> refresh_policy::PoolRefresh {
        self.preferences()
            .map(|p| p.refresh)
            .unwrap_or_default()
            .pool(pool)
    }
    /// The interval of the pool's rounds: as set, or sooner for now while
    /// an adaptive pool's usage changes.
    pub(crate) fn round_interval(&self, pool: Pool) -> Duration {
        let refresh = self.pool_refresh(pool);
        self.schedule(pool)
            .filter(|_| refresh.adaptive)
            .map(|s| Duration::from_secs(s.interval as u64))
            .unwrap_or(refresh.interval())
    }
    /// Applies a pool's new refresh to its next round, which waits the new
    /// interval from now.
    pub(crate) fn reschedule(&self, pool: Pool, refresh: refresh_policy::PoolRefresh) {
        let rounds = self.rounds(pool);
        *rounds.unchanged.lock().unwrap() = 0;
        let now = now_unix_seconds();
        let waiting = {
            let mut schedule = rounds.schedule.lock().unwrap();
            match schedule.as_mut() {
                Some(next) => {
                    next.interval = refresh.seconds as i64;
                    next.full_every = refresh.full_every;
                    // A round under way schedules the next itself.
                    let waiting = next.at > now;
                    if waiting {
                        next.at = now + next.interval;
                    }
                    waiting
                }
                None => false,
            }
        };
        if waiting {
            rounds.deferred.notify_one();
        }
        self.publish();
    }
    /// Refreshing the whole pool stands in for a background round: its next
    /// round waits a whole interval from now. A round under way is left to
    /// schedule the next itself.
    pub fn defer_background_round(&self, pool: Pool) {
        let rounds = self.rounds(pool);
        let now = now_unix_seconds();
        if let Some(next) = rounds
            .schedule
            .lock()
            .unwrap()
            .as_mut()
            .filter(|s| s.at > now)
        {
            next.at = now + next.interval;
            // Kept until the loop waits, should it not wait yet.
            rounds.deferred.notify_one();
        }
    }
    pub(crate) async fn background_round(&self, tick: u64, pool: Pool) -> Duration {
        let refresh = self.pool_refresh(pool);
        let interval = self.round_interval(pool);
        let set_schedule = |at, tick, interval: Duration, full_every| {
            *self.rounds(pool).schedule.lock().unwrap() = Some(Schedule {
                at,
                tick,
                interval: interval.as_secs() as i64,
                full_every,
            });
        };
        set_schedule(now_unix_seconds(), tick, interval, refresh.full_every);
        let before = if refresh.adaptive {
            Some(self.usage_values(pool).await)
        } else {
            None
        };
        // Access to the Claude pool is asked for once, before its first round.
        if pool == Pool::Claude && tick == 0 {
            let _ = self.claude_authorize_at_startup().await;
        }
        let _ = self.refresh_recurring(pool, tick).await;
        // A refresh set while the round ran applies from the next round:
        // `reschedule` leaves a round under way to schedule it.
        let latest = self.pool_refresh(pool);
        let interval = match before {
            _ if latest != refresh => latest.interval(),
            Some(before) => {
                let changed =
                    refresh_policy::usage_changed(&before, &self.usage_values(pool).await);
                refresh_policy::refresh_interval_for(
                    refresh.interval(),
                    interval,
                    changed,
                    &mut self.rounds(pool).unchanged.lock().unwrap(),
                )
            }
            None => refresh.interval(),
        };
        set_schedule(
            now_unix_seconds() + interval.as_secs() as i64,
            tick.wrapping_add(1),
            interval,
            latest.full_every,
        );
        self.publish();
        interval
    }
    /// Each card's usage, to tell whether a round changed it. Claude's
    /// cards are read without the login Claude Code uses, which is in the
    /// Keychain.
    async fn usage_values(&self, pool: Pool) -> std::collections::BTreeMap<String, Value> {
        let cards = match pool {
            Pool::Claude => self.claude_usage_cards(),
            _ => self.accounts(pool).await,
        };
        refresh_policy::usage_values(&cards.unwrap_or_default())
    }
    /// The pool's next background round; none before the first starts.
    pub(crate) fn schedule(&self, pool: Pool) -> Option<Schedule> {
        *self.rounds(pool).schedule.lock().unwrap()
    }
    #[cfg(test)]
    pub(crate) fn set_schedule(&self, pool: Pool, schedule: Schedule) {
        *self.rounds(pool).schedule.lock().unwrap() = Some(schedule);
    }
    /// When the next background round that refreshes the whole pool starts
    /// (Unix seconds), should the interval stay as it is; none before the
    /// first round.
    pub fn next_full_refresh_at(&self, pool: Pool) -> Option<i64> {
        self.schedule(pool).map(|s| s.next_refresh_at(false, 0))
    }
}
