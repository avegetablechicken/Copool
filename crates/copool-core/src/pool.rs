//! The account pools, Codex, Claude Code, Antigravity and Grok, behind one set of
//! operations.
//! Each pool keeps only what is its own, supplied through `PoolOps`: where
//! its logins are stored and which login its tool uses, how a login is
//! renewed, and how usage is asked for. Everything done with the cards
//! (editing one, choosing the best, switching automatically, renewing at
//! once, saying hi, refreshing by hand and in background rounds) is
//! implemented here once, for every pool.
/// A pool's request to its service or tool, answered later.
pub(crate) type Response<'a, T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = std::result::Result<T, String>> + Send + 'a>,
>;
use crate::{
    application::{eligible, Application, SmartSwitchOutcome, SwitchExecution},
    models::{AccountSummary, Renewal, StoredAccount},
    plan_tags, refresh_plan,
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

/// A pool's edit of its stored accounts, which reports whether it changed
/// them.
pub(crate) type Edit<'a> = Box<dyn FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool> + Send + 'a>;

/// What a pool supplies to the operations here, the same for every pool:
/// each pool's `LoginPool` adapter implements it in `<pool>.rs`, and
/// `Application::ops` alone picks it by `Pool`.
pub(crate) trait PoolOps: Send + Sync {
    /// The pool's cards, following the login its tool uses: its accounts and
    /// its Sub2api providers' cards, sorted for display. `accounts` adds
    /// their tags and refresh.
    fn cards(&self) -> Response<'_, Vec<AccountSummary>>;
    /// The cards whose usage a background round compares; `cards` unless
    /// reading those would ask for the Keychain.
    fn usage_cards(&self) -> Response<'_, Vec<AccountSummary>> {
        self.cards()
    }
    /// The Sub2api provider the pool's tool uses, if any.
    fn provider_in_use(&self) -> Option<String>;
    /// Until when a rate limit pauses each account's requests, by id; none
    /// unless the pool's service pauses them.
    fn refresh_pauses(&self) -> std::collections::BTreeMap<String, i64> {
        Default::default()
    }
    /// Asks for the Keychain access a user action may ask for and
    /// background work does not; nothing for a pool that needs none.
    fn authorize(&self) -> Response<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    /// Asks for the access its background rounds need, once, before the
    /// first; nothing for a pool that needs none.
    fn authorize_at_startup(&self) -> Response<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    /// Imports the login the pool's tool uses; returns how many accounts.
    fn import_current(&self) -> Response<'_, usize>;
    /// Imports exported logins, the whole document checked before any is
    /// stored; returns how many.
    fn import_json<'a>(&'a self, content: &'a str) -> Response<'a, usize>;
    /// Stores a browser sign-in's login, of the workspace it was asked for
    /// if any, without switching to it.
    fn import_sign_in<'a>(
        &'a self,
        document: Value,
        workspace: Option<&'a str>,
    ) -> Response<'a, usize>;
    /// Removes an account.
    fn delete<'a>(&'a self, id: &'a str) -> Response<'a, ()>;
    /// Edits the stored accounts, saved when `edit` reports a change.
    fn edit<'a>(&'a self, edit: Edit<'a>) -> Response<'a, ()>;
    /// Gives the pool's tool this card's login, or its Sub2api provider.
    fn switch<'a>(&'a self, id: &'a str, access: KeychainAccess) -> Response<'a, SwitchExecution>;
    /// Whether the account failed: its usage refused, or its token expired.
    fn login_failed<'a>(&'a self, id: &'a str) -> Response<'a, bool>;
    /// Refreshes an account's usage, its login renewed as its renewal
    /// allows; with `renew_now`, only renews it, whatever the choice, and
    /// reports whether it did.
    fn refresh<'a>(&'a self, id: &'a str, renew_now: Option<&'a mut bool>) -> Response<'a, ()>;
    /// One hi with an account, without switching to it; `third_party` as
    /// `say_hi_to` takes it.
    fn hello<'a>(&'a self, id: &'a str, third_party: Option<bool>) -> Response<'a, Hello>;
}

/// One pool's background refresh rounds.
#[derive(Default)]
pub(crate) struct Rounds {
    /// The next round; none before the first starts.
    schedule: std::sync::Mutex<Option<Schedule>>,
    /// Its wait, started over by refreshing the whole pool.
    deferred: tokio::sync::Notify,
    /// The cards speeding up, by card key, with their unchanged refreshes in
    /// a row since.
    fast: std::sync::Mutex<std::collections::BTreeMap<String, u8>>,
    /// When the cards speeding up are next asked for.
    fast_at: std::sync::Mutex<Option<i64>>,
    /// Wakes the speed-up loop when a card starts speeding up.
    sped_up: tokio::sync::Notify,
}

impl Application {
    /// The pool's own parts, which the operations here reach it through.
    pub(crate) fn ops(&self, pool: Pool) -> Box<dyn PoolOps + '_> {
        match pool {
            Pool::Codex => crate::codex::ops(self),
            Pool::Claude => crate::claude::ops(self),
            Pool::Antigravity => crate::antigravity::ops(self),
            Pool::Grok => crate::grok::ops(self),
        }
    }
    fn rounds(&self, pool: Pool) -> &Rounds {
        &self.rounds[pool as usize]
    }
    /// The pool's cards, following the login its tool uses, each with its
    /// plan and workspace tags and how the background refresh treats it:
    /// when it next asks for the card (`refresh_plan`).
    pub async fn accounts(&self, pool: Pool) -> Result<Vec<AccountSummary>> {
        let mut accounts = self.ops(pool).cards().await?;
        let now = now_unix_seconds();
        let refresh = self.preferences().map(|p| p.refresh).unwrap_or_default();
        let schedule = self.schedule(pool);
        let pauses = self.refresh_pauses(pool);
        let next: Vec<Option<i64>> = accounts
            .iter()
            .map(|account| {
                let card = refresh_plan::Card::of(pool, account, &pauses);
                let schedule = schedule.as_ref()?;
                refresh_plan::next_refresh_at(&card, schedule, self.speedup(pool, &account.id))
            })
            .collect();
        for (account, next) in accounts.iter_mut().zip(next) {
            account.tags = plan_tags::of(pool, account, now);
            account.next_refresh_at = next;
            account.refresh = Some(refresh_plan::summary(pool, account, &refresh));
        }
        Ok(accounts)
    }
    /// Until when a rate limit pauses each account's requests, by id.
    fn refresh_pauses(&self, pool: Pool) -> std::collections::BTreeMap<String, i64> {
        self.ops(pool).refresh_pauses()
    }
    /// Lets a user action ask for the Keychain access background work does
    /// not, as after a build changed Copool's signature.
    pub async fn authorize(&self, pool: Pool) -> Result<()> {
        self.ops(pool).authorize().await
    }
    /// Imports the login the pool's tool uses; returns how many accounts.
    pub async fn import_current(&self, pool: Pool) -> Result<usize> {
        self.ops(pool).import_current().await
    }
    /// Imports exported logins, the whole document validated before any is
    /// stored; returns how many.
    pub async fn import_json(&self, pool: Pool, content: &str) -> Result<usize> {
        if content.len() as u64 > IMPORT_LIMIT {
            return Err("Import exceeds 16 MiB".into());
        }
        self.ops(pool).import_json(content).await
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
        self.ops(pool).import_sign_in(document, workspace).await
    }
    /// Removes a card; a Sub2api card leaves its provider until a sync.
    pub async fn delete_account(&self, pool: Pool, id: &str) -> Result<()> {
        if sub2api::is_card(pool, id) {
            return self.delete_card(pool, id).await;
        }
        self.ops(pool).delete(id).await
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
    /// Sets whether Copool renews an account's tokens: "never" or "always".
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
        self.ops(pool).edit(Box::new(edit)).await
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
        self.ops(pool).switch(id, access).await
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
        self.ops(pool).refresh(id, renew_now).await
    }
    /// Renews an account's tokens at once, whatever its renewal choice, which
    /// stays as it was. Usage is not asked for, unless the account had
    /// failed: its token expired or its usage refused; it then refreshes once
    /// renewed, and a usage error shows on the card.
    pub async fn renew_now(&self, pool: Pool, id: &str) -> Result<()> {
        if sub2api::is_card(pool, id) {
            return Err("Token renewal is not available for this account".into());
        }
        let failed = self.ops(pool).login_failed(id).await?;
        self.release_new_accounts(pool, Some(id)).await?;
        let mut renewed = false;
        let result = self.refresh_login(pool, id, Some(&mut renewed)).await;
        if !renewed {
            result?;
            return Err("The token was not renewed".into());
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
        self.say_hi_to(pool, id, None).await
    }
    /// Says hi as `say_hi` does, to the card's own pool alone, or with
    /// `third_party` to its third-party pool alone, as the card's model
    /// switch has chosen; only an Antigravity account has the latter.
    pub async fn say_hi_to(&self, pool: Pool, id: &str, third_party: Option<bool>) -> Result<()> {
        if third_party == Some(true) && (pool != Pool::Antigravity || sub2api::is_card(pool, id)) {
            return Err("This card has no third-party pool to say hi to".into());
        }
        let _greeting = self.begin_greeting(pool, id)?;
        if sub2api::is_card(pool, id) {
            return self.say_hi_through_provider(pool, id).await;
        }
        let mut renewed = false;
        loop {
            let hello = self.ops(pool).hello(id, third_party).await?;
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
    /// use, the whole pool and every provider on its full rounds
    /// (`refresh_plan::round`).
    pub async fn refresh_recurring(&self, pool: Pool, tick: u64) -> Result<()> {
        let accounts = self.accounts(pool).await?;
        let pauses = self.refresh_pauses(pool);
        let cards: Vec<_> = accounts
            .iter()
            .map(|account| refresh_plan::Card::of(pool, account, &pauses))
            .collect();
        let full = refresh_policy::is_full_round(tick, self.pool_refresh(pool).full_every);
        let interval = self.round_interval(pool).as_secs() as i64;
        let in_use = self.ops(pool).provider_in_use();
        let ask = refresh_plan::round(
            &cards,
            in_use.as_deref(),
            full,
            now_unix_seconds(),
            interval,
        );
        self.refresh_pool(pool, Some(&ask.ids), ask.providers.as_deref())
            .await
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
            self.background_pool(Pool::Grok),
            self.background_fast(Pool::Codex),
            self.background_fast(Pool::Claude),
            self.background_fast(Pool::Antigravity),
            self.background_fast(Pool::Grok),
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
    /// Between rounds, the cards speeding up are asked for on their own,
    /// sooner, until their usage stops changing.
    async fn background_fast(&self, pool: Pool) {
        loop {
            let wait = self.fast_wait(pool);
            match wait {
                Some(wait) => {
                    tokio::time::sleep(wait).await;
                    self.fast_round(pool).await;
                }
                None => self.rounds(pool).sped_up.notified().await,
            }
        }
    }
    /// The wait for the next ask for the cards speeding up, from now; none
    /// while none does.
    pub(crate) fn fast_wait(&self, pool: Pool) -> Option<Duration> {
        let rounds = self.rounds(pool);
        let wait = (!rounds.fast.lock().unwrap().is_empty())
            .then(|| refresh_policy::fast_interval(self.pool_refresh(pool).interval()));
        *rounds.fast_at.lock().unwrap() =
            wait.map(|wait| now_unix_seconds() + wait.as_secs() as i64);
        self.publish();
        wait
    }
    /// Asks for the cards speeding up that were not asked for within the
    /// sooner interval, then notes which changed. Each such ask counts
    /// toward the unchanged refreshes that end a card's speed-up.
    pub(crate) async fn fast_round(&self, pool: Pool) {
        let refresh = self.preferences().map(|p| p.refresh).unwrap_or_default();
        let keys: Vec<String> = {
            let mut fast = self.rounds(pool).fast.lock().unwrap();
            fast.retain(|key, _| refresh.card(pool, key).adaptive);
            fast.keys().cloned().collect()
        };
        *self.rounds(pool).fast_at.lock().unwrap() = None;
        if keys.is_empty() {
            return;
        }
        let soon = refresh_policy::fast_interval(refresh.pool(pool).interval()).as_secs() as i64;
        let accounts = self.accounts(pool).await.unwrap_or_default();
        let pauses = self.refresh_pauses(pool);
        let cards: Vec<_> = accounts
            .iter()
            .map(|account| refresh_plan::Card::of(pool, account, &pauses))
            .collect();
        let ask = refresh_plan::fast(pool, &cards, &keys, now_unix_seconds(), soon);
        let (ids, providers) = (ask.ids, ask.providers.unwrap_or_default());
        let before = self.card_usage(pool).await;
        if !ids.is_empty() || !providers.is_empty() {
            let _ = self.refresh_pool(pool, Some(&ids), Some(&providers)).await;
        }
        let changed = self.changed_cards(pool, &before).await;
        let mut fast = self.rounds(pool).fast.lock().unwrap();
        for key in &keys {
            refresh_policy::track_speed_up(&mut fast, key, true, changed.contains(key), true);
        }
    }
    /// Starts a card's speed-up for each changed card whose own is on, and
    /// ends those turned off.
    fn note_changes(
        &self,
        pool: Pool,
        refresh: &refresh_policy::RefreshPreferences,
        changed: &std::collections::BTreeSet<String>,
    ) {
        let rounds = self.rounds(pool);
        let mut fast = rounds.fast.lock().unwrap();
        let was_empty = fast.is_empty();
        let keys: std::collections::BTreeSet<String> =
            fast.keys().chain(changed).cloned().collect();
        for key in keys {
            let adaptive = refresh.card(pool, &key).adaptive;
            refresh_policy::track_speed_up(
                &mut fast,
                &key,
                adaptive,
                changed.contains(&key),
                false,
            );
        }
        if was_empty && !fast.is_empty() {
            rounds.sped_up.notify_one();
        }
    }
    /// The asks between rounds while the card speeds up; none for a card
    /// that does not.
    pub(crate) fn speedup(&self, pool: Pool, id: &str) -> Option<refresh_plan::Speedup> {
        let rounds = self.rounds(pool);
        if !rounds
            .fast
            .lock()
            .unwrap()
            .contains_key(&refresh_policy::card_key(pool, id))
        {
            return None;
        }
        let at = (*rounds.fast_at.lock().unwrap())?;
        let soon = refresh_policy::fast_interval(self.round_interval(pool)).as_secs() as i64;
        Some(refresh_plan::Speedup { at, soon })
    }
    /// The pool's background refresh, as set for all its cards.
    pub(crate) fn pool_refresh(&self, pool: Pool) -> refresh_policy::PoolRefresh {
        self.preferences()
            .map(|p| p.refresh)
            .unwrap_or_default()
            .pool(pool)
    }
    /// A card's refresh: its pool's, with the card's own switches.
    pub(crate) fn card_refresh(&self, pool: Pool, id: &str) -> refresh_policy::PoolRefresh {
        self.preferences()
            .map(|p| p.refresh)
            .unwrap_or_default()
            .card(pool, &refresh_policy::card_key(pool, id))
    }
    /// The interval of the pool's rounds, as set; a card speeding up is
    /// asked for between them.
    pub(crate) fn round_interval(&self, pool: Pool) -> Duration {
        self.pool_refresh(pool).interval()
    }
    /// Applies a pool's new refresh to its next round, which waits the new
    /// interval from now.
    pub(crate) fn reschedule(&self, pool: Pool, refresh: refresh_policy::PoolRefresh) {
        let rounds = self.rounds(pool);
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
        let interval = refresh.interval();
        let set_schedule = |at, tick, interval: Duration, full_every| {
            *self.rounds(pool).schedule.lock().unwrap() = Some(Schedule {
                at,
                tick,
                interval: interval.as_secs() as i64,
                full_every,
            });
        };
        set_schedule(now_unix_seconds(), tick, interval, refresh.full_every);
        let before = self.card_usage(pool).await;
        if tick == 0 {
            let _ = self.ops(pool).authorize_at_startup().await;
        }
        let _ = self.refresh_recurring(pool, tick).await;
        // A refresh set while the round ran applies from the next round:
        // `reschedule` leaves a round under way to schedule it.
        let changed = self.changed_cards(pool, &before).await;
        let preferences = self.preferences().map(|p| p.refresh).unwrap_or_default();
        self.note_changes(pool, &preferences, &changed);
        let latest = preferences.pool(pool);
        let interval = latest.interval();
        set_schedule(
            now_unix_seconds() + interval.as_secs() as i64,
            tick.wrapping_add(1),
            interval,
            latest.full_every,
        );
        self.publish();
        interval
    }
    /// Each account's usage, to tell whether a refresh changed it.
    async fn card_usage(&self, pool: Pool) -> std::collections::BTreeMap<String, Value> {
        let cards = self.ops(pool).usage_cards().await;
        refresh_policy::usage_values(&cards.unwrap_or_default())
    }
    /// The keys of the cards whose usage changed since `before`.
    async fn changed_cards(
        &self,
        pool: Pool,
        before: &std::collections::BTreeMap<String, Value>,
    ) -> std::collections::BTreeSet<String> {
        let after = self.card_usage(pool).await;
        refresh_policy::changed_cards(before, &after)
            .map(|id| refresh_policy::card_key(pool, id))
            .collect()
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
