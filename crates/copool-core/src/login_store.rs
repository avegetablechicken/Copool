//! The account store of a pool whose logins Copool keeps in a file of its
//! own and whose tool signs in with one Live login: Codex (its `auth.json`),
//! Grok (Grok Build's `auth.json`) and Antigravity (agy's item in the
//! system's credential store). The store follows the Live login, imports logins, deletes and
//! edits accounts, makes the switch's write to the Live login, and renews
//! tokens: it uses the Live login's renewal when the tool made one, and
//! puts its own back there. Each pool supplies only its facts, as a
//! [`LoginPool`]: where its Live login is and how it is read and written,
//! how its login is read, how a token is renewed and which account a login
//! is. Its switch effects, usage requests and Sub2api providers stay its own.
//!
//! One locking rule holds for every pool: the Live login is read and
//! written outside the store's lock, since reaching it may wait for the
//! user to allow it, and a read taken before a switch is not followed after
//! it. A pool without accounts never reads its Live login.
//!
//! An account its issuer deactivated stays listed so ([`Standing`]); deleted,
//! it stays hidden, for the pool to remember the workspace gone.
//!
//! Every pool is meant to keep its accounts here; Claude has yet to: its
//! login (its tokens and its `oauthAccount`), its vault and Claude Code's
//! refresh lock are to be its pool's facts.
use crate::{
    application::Application,
    models::{AccountSummary, Renewal, UsageSnapshot, RENEWAL_OFF},
    persistence::write_json,
    pool::PoolAccount,
    utils::now_unix_seconds,
    vault::KeychainAccess,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    future::Future,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

type Result<T> = std::result::Result<T, String>;

/// An access token this close to its expiry is renewed first.
pub(crate) const EXPIRY_MARGIN: i64 = 60;

/// A pool's locks, part of its state.
#[derive(Default)]
pub(crate) struct Locks {
    /// Held across a load and save of the store.
    pub(crate) store: tokio::sync::Mutex<()>,
    /// Serializes imports, switches and refreshes.
    pub(crate) operations: tokio::sync::Mutex<()>,
    /// Counts switches made: a Live login read before one is not followed
    /// after it.
    switches: AtomicU64,
}

/// An account of the pool: what every pool keeps of it, and what its own
/// pool adds (`own`), saved beside it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account<O> {
    pub(crate) id: String,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) email: Option<String>,
    /// The login as the tool's Live login holds it.
    pub(crate) document: Value,
    #[serde(flatten)]
    pub(crate) own: O,
    #[serde(default)]
    pub(crate) proxy_url: String,
    pub(crate) added_at: i64,
    pub(crate) updated_at: i64,
    #[serde(default)]
    pub(crate) usage: Option<UsageSnapshot>,
    #[serde(default)]
    pub(crate) usage_error: Option<String>,
    #[serde(default)]
    pub(crate) plan: Option<String>,
    /// Newly imported; nothing goes upstream for it until a manual refresh.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) first_request_pending: bool,
    /// The user's choice of whether to renew the tokens; none for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) renewal: Option<Renewal>,
    /// Deactivated by its issuer, or deleted once it was; none for an
    /// account in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) standing: Option<Standing>,
}
/// Why an account can no longer be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Standing {
    /// Its issuer reports it deactivated: it stays listed so.
    Deactivated,
    /// Deleted while deactivated: kept, hidden, so that its workspace is
    /// known to be gone.
    Deleted,
}
/// What a pool adds to its accounts when it adds nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Nothing {}

impl<O> Account<O> {
    /// Records a usage request's outcome: its usage, with the account's
    /// plan, or its error; nothing for a renewal alone.
    pub(crate) fn record(&mut self, outcome: &Result<Option<UsageSnapshot>>, now: i64) {
        match outcome {
            Ok(Some(usage)) => {
                let mut usage = usage.clone();
                usage.plan_type = self.plan.clone();
                self.usage = Some(usage);
                self.usage_error = None;
                self.updated_at = now;
            }
            Ok(None) => {}
            Err(error) => {
                self.usage_error = Some(error.clone());
                self.updated_at = now;
            }
        }
    }
}
impl<O> PoolAccount for Account<O> {
    fn id(&self) -> &str {
        &self.id
    }
    fn set_renewal(&mut self, renewal: Renewal) {
        self.renewal = Some(renewal);
    }
    fn set_proxy(&mut self, proxy: String) {
        self.proxy_url = proxy;
    }
    fn release(&mut self) -> bool {
        std::mem::take(&mut self.first_request_pending)
    }
}

/// A pool's facts, which its store works from.
pub(crate) trait LoginPool: Sync {
    /// What the pool adds to its accounts.
    type Own: Clone + Default + Serialize + DeserializeOwned + Send + Sync;
    /// The pool's file: its accounts, the account current, and the rest
    /// the pool keeps there.
    type Store: Default + Serialize + DeserializeOwned + Send;
    /// A login as the Live login holds it.
    type Login: Clone + Send + Sync;
    /// A renewal of an access token.
    type Renewed: Send + Sync;

    /// The store's file in Copool's data directory.
    const FILE: &'static str;
    /// The pool's name in errors.
    const NAME: &'static str;
    /// The label of an account whose login names no one.
    const UNNAMED: &'static str;
    /// The error of a login its issuer no longer renews.
    const SIGN_IN_EXPIRED: &'static str;
    /// The error of an account the tool may use, not renewed since its Live
    /// login cannot be read: the new tokens could not reach it, and the
    /// tool's would stop working.
    const LIVE_UNREADABLE: &'static str;

    fn app(&self) -> &Application;
    fn locks(&self) -> &Locks;
    fn accounts(store: &Self::Store) -> &Vec<Account<Self::Own>>;
    fn accounts_mut(store: &mut Self::Store) -> &mut Vec<Account<Self::Own>>;
    fn current(store: &mut Self::Store) -> &mut Option<String>;

    /// The Live login: none when there is none; an error when it cannot be
    /// read or is no login.
    fn read_live(
        &self,
        access: KeychainAccess,
    ) -> impl Future<Output = Result<Option<Self::Login>>> + Send;
    fn write_live(
        &self,
        login: &Self::Login,
        access: KeychainAccess,
    ) -> impl Future<Output = Result<()>> + Send;
    fn document(login: &Self::Login) -> &Value;
    fn document_mut(login: &mut Self::Login) -> &mut Value;
    /// The account's login, as the Live login would hold it.
    fn login_of(account: &Account<Self::Own>) -> Self::Login;
    /// Keeps `login` as the account's.
    fn keep(account: &mut Account<Self::Own>, login: Self::Login);

    fn access_token(document: &Value) -> Option<String>;
    fn refresh_token(document: &Value) -> Option<String>;
    /// When the access token expires, in seconds.
    fn expiry(document: &Value) -> Option<i64>;
    fn email(document: &Value) -> Option<String>;
    /// The label of a new account: its email, unless the pool names it
    /// otherwise.
    fn name(document: &Value) -> Option<String> {
        Self::email(document)
    }
    /// Whether two documents are generations of one login.
    fn same_login(a: &Value, b: &Value) -> bool;
    /// Whether `a` is a later generation than `b`: by default, its access
    /// token lasts longer.
    fn newer(a: &Value, b: &Value) -> bool {
        Self::expiry(a).unwrap_or(0) > Self::expiry(b).unwrap_or(0)
    }
    /// Whether a request's error says the account was deactivated.
    fn deactivated(_error: &str) -> bool {
        false
    }
    /// What the pool adds to the account's card.
    fn summarize(_account: &Account<Self::Own>, _summary: &mut AccountSummary) {}
    /// Whether a login is the account's.
    fn same_account(account: &Account<Self::Own>, document: &Value) -> bool {
        Self::same_login(&account.document, document)
    }
    /// The account's id at its issuer, for its card.
    fn account_id(account: &Account<Self::Own>) -> String;

    /// Renews the document's access token in place; an error when the
    /// issuer refuses.
    fn renew(
        &self,
        document: &mut Value,
        proxy: &str,
    ) -> impl Future<Output = Result<Self::Renewed>> + Send;
    fn apply(renewed: &Self::Renewed, document: &mut Value);

    /// Puts back, once the store is read, what the pool keeps of its
    /// accounts elsewhere (Claude's vault): nothing by default.
    fn opened(&self, _store: &mut Self::Store) -> Result<()> {
        Ok(())
    }
    /// The store as its file keeps it, what the pool keeps elsewhere put
    /// there first: all of it by default.
    fn sealed(&self, store: &Self::Store) -> Result<Value> {
        serde_json::to_value(store).map_err(|e| e.to_string())
    }
    /// The tool's lock on renewing its Live login, held from a renewal's
    /// reread of the Live login until the renewal reaches it, so that the
    /// tool and Copool never both spend one refresh token: none by default.
    fn renewal_lock(&self) -> impl Future<Output = Result<Option<Box<dyn RenewalLock>>>> + Send {
        async { Ok(None) }
    }
}

/// A tool's lock on renewing its login, while held.
pub(crate) trait RenewalLock: Send {
    /// An error once another program may be renewing too.
    fn held(&self) -> Result<()>;
}

/// How a renewal gave a fresh token.
enum Renewing {
    Renewed,
    /// The tool renewed the login while the renewal waited for its lock.
    Adopted,
}

/// Whether two documents share a token.
fn shares_token<P: LoginPool>(a: &Value, b: &Value) -> bool {
    let same = |token: fn(&Value) -> Option<String>| token(a).is_some() && token(a) == token(b);
    same(P::access_token) || same(P::refresh_token)
}

/// What a usage request said of the token it was given.
pub(crate) enum Asked<T> {
    Answered(T),
    /// The token was refused: renewing it may help.
    Refused,
    Failed(String),
}

/// A refresh's token, as [`Logins::use_token`] left it.
pub(crate) struct Used<T> {
    /// What the request answered; none when the token was only renewed.
    pub(crate) outcome: Result<Option<T>>,
    /// The login whose token was used last.
    pub(crate) document: Value,
    pub(crate) renewed: bool,
    /// Taken from the Live login, which the tool renewed.
    pub(crate) adopted: bool,
}

/// A refresh's start: the account as followed, and whether it may be the
/// one the tool uses while the Live login could not be read.
pub(crate) struct Begun<O> {
    pub(crate) account: Account<O>,
    pub(crate) live_unreadable: bool,
}

/// The store of a pool.
pub(crate) struct Logins<P>(pub(crate) P);

impl<P: LoginPool> Logins<P> {
    fn path(&self) -> PathBuf {
        self.0.app().paths.data_dir.join(P::FILE)
    }
    pub(crate) fn load(&self) -> Result<P::Store> {
        let mut store = match fs::read(self.path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid {} account store: {e}", P::NAME))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => P::Store::default(),
            Err(e) => return Err(e.to_string()),
        };
        self.0.opened(&mut store)?;
        Ok(store)
    }
    pub(crate) fn save(&self, store: &P::Store) -> Result<()> {
        write_json(&self.path(), &self.0.sealed(store)?)?;
        self.0.app().publish();
        Ok(())
    }

    /// The account's card.
    pub(crate) fn summary(account: &Account<P::Own>, current: bool) -> AccountSummary {
        let (display, workspace) = match account.standing {
            None => ("list", "active"),
            Some(Standing::Deactivated) => ("deactivated", "deactivated"),
            Some(Standing::Deleted) => ("deleted", "deactivated"),
        };
        let mut summary = AccountSummary {
            display_status: display.into(),
            workspace_status: workspace.into(),
            source_tag: None,
            team_name: None,
            available: account.standing.is_none(),
            proxy_url: account.proxy_url.clone(),
            id: account.id.clone(),
            label: account.label.clone(),
            email: account.email.clone(),
            account_id: P::account_id(account),
            plan_type: account.plan.clone(),
            added_at: account.added_at,
            updated_at: account.updated_at,
            usage: account.usage.clone(),
            usage_error: account.usage_error.clone(),
            is_current: current,
            awaiting_first_request: account.first_request_pending,
            token_expires_at: P::expiry(&account.document)
                .and_then(crate::models::displayed_expiry),
            renewal: Some(Renewal::of(account.renewal).as_str().into()),
            next_refresh_at: None,
            refresh: None,
            outdated_sub2api_version: None,
            tags: Default::default(),
        };
        P::summarize(account, &mut summary);
        summary
    }
    /// Records a usage request's outcome in the account, as
    /// [`Account::record`]; an error saying the account was deactivated
    /// marks it so, and usage marks it in use again unless deleted.
    pub(crate) fn record_usage(
        account: &mut Account<P::Own>,
        outcome: &Result<Option<UsageSnapshot>>,
        now: i64,
    ) {
        account.record(outcome, now);
        match outcome {
            Ok(Some(_)) if account.standing == Some(Standing::Deactivated) => {
                account.standing = None;
            }
            Err(error) if P::deactivated(error) && account.standing.is_none() => {
                account.standing = Some(Standing::Deactivated);
            }
            _ => {}
        }
    }

    /// Takes a newer generation of the account's login, which the tool
    /// renewed. Returns whether it did.
    fn adopt(account: &mut Account<P::Own>, login: &P::Login) -> bool {
        let document = P::document(login);
        let same_tokens = P::access_token(document) == P::access_token(&account.document)
            && P::refresh_token(document) == P::refresh_token(&account.document);
        if !P::newer(document, &account.document) || same_tokens {
            return false;
        }
        P::keep(account, login.clone());
        account.usage_error = None;
        true
    }
    /// Follows the Live login: its account becomes current, and a newer
    /// generation of it is adopted. None read leaves the selection as last
    /// seen, as does a login holding another account's tokens: read while a
    /// switch wrote it, it pairs one account's tokens with another's name.
    /// Returns whether the store changed.
    pub(crate) fn follow_live(store: &mut P::Store, live: Option<&P::Login>) -> bool {
        let Some(live) = live else {
            return false;
        };
        let document = P::document(live);
        let accounts = P::accounts(store);
        let found = accounts.iter().position(|a| P::same_account(a, document));
        if let Some(i) = found {
            let others = accounts.iter().enumerate().filter(|(j, _)| *j != i);
            if others
                .into_iter()
                .any(|(_, a)| shares_token::<P>(&a.document, document))
            {
                return false;
            }
        }
        let mut changed = false;
        let current = found.map(|i| {
            let account = &mut P::accounts_mut(store)[i];
            changed |= Self::adopt(account, live);
            account.id.clone()
        });
        if *P::current(store) != current {
            *P::current(store) = current;
            changed = true;
        }
        changed
    }
    /// The store, having followed the Live login.
    pub(crate) async fn followed(&self) -> Result<P::Store> {
        let locks = self.0.locks();
        let switches = locks.switches.load(Ordering::SeqCst);
        let live = if P::accounts(&self.load()?).is_empty() {
            None
        } else {
            self.0
                .read_live(KeychainAccess::Background)
                .await
                .ok()
                .flatten()
        };
        let _lock = locks.store.lock().await;
        let mut store = self.load()?;
        // A switch made since may have replaced the login read.
        if locks.switches.load(Ordering::SeqCst) == switches
            && Self::follow_live(&mut store, live.as_ref())
        {
            self.save(&store)?;
        }
        Ok(store)
    }

    /// Adds or updates the account of a login; returns its id and whether
    /// it waits for a manual refresh.
    async fn upsert(&self, login: P::Login, current: bool) -> Result<(String, bool)> {
        let defer = !self.0.app().preferences()?.request_new_accounts;
        let now = now_unix_seconds();
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        let email = P::email(P::document(&login));
        let name = P::name(P::document(&login));
        let found = P::accounts_mut(&mut store)
            .iter_mut()
            .find(|a| P::same_account(a, P::document(&login)));
        let (id, waiting) = match found {
            Some(existing) => {
                P::keep(existing, login);
                existing.updated_at = now;
                existing.usage_error = None;
                // Signed in again: in use, unless the user deleted it.
                if existing.standing == Some(Standing::Deactivated) {
                    existing.standing = None;
                }
                if let Some(email) = email {
                    existing.label = email.clone();
                    existing.email = Some(email);
                }
                (existing.id.clone(), existing.first_request_pending)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let mut account = Account {
                    id: id.clone(),
                    label: name.unwrap_or_else(|| P::UNNAMED.into()),
                    email,
                    document: Value::Null,
                    own: P::Own::default(),
                    proxy_url: String::new(),
                    added_at: now,
                    updated_at: now,
                    usage: None,
                    usage_error: None,
                    plan: None,
                    first_request_pending: defer,
                    renewal: None,
                    standing: None,
                };
                P::keep(&mut account, login);
                P::accounts_mut(&mut store).push(account);
                (id, defer)
            }
        };
        if current {
            *P::current(&mut store) = Some(id.clone());
        }
        self.save(&store)?;
        Ok((id, waiting))
    }
    /// Imports logins, checked whole before, without switching to them;
    /// returns the accounts to refresh now, the others waiting for a manual
    /// refresh.
    pub(crate) async fn import(&self, logins: Vec<P::Login>) -> Result<Vec<String>> {
        let _op = self.0.locks().operations.lock().await;
        let mut fresh = Vec::new();
        for login in logins {
            let (id, waiting) = self.upsert(login, false).await?;
            if !waiting {
                fresh.push(id);
            }
        }
        Ok(fresh)
    }
    /// Imports the Live login, which becomes current; `not_found` when there
    /// is none. Returns it when it is to be refreshed now.
    pub(crate) async fn import_live(
        &self,
        access: KeychainAccess,
        not_found: &str,
    ) -> Result<Option<String>> {
        let _op = self.0.locks().operations.lock().await;
        let live = self.0.read_live(access).await?.ok_or(not_found)?;
        let (id, waiting) = self.upsert(live, true).await?;
        Ok((!waiting).then_some(id))
    }
    /// Changes the store under its lock, saving it.
    pub(crate) async fn change<T>(
        &self,
        change: impl FnOnce(&mut P::Store) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        let changed = change(&mut store)?;
        self.save(&store)?;
        Ok(changed)
    }
    /// Removes an account. A deactivated one stays, deleted, for `kept` to
    /// record where the pool keeps track of what is gone.
    pub(crate) async fn delete(
        &self,
        id: &str,
        kept: impl FnOnce(&mut P::Store, usize),
    ) -> Result<()> {
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        let accounts = P::accounts_mut(&mut store);
        let Some(index) = accounts.iter().position(|a| a.id == id) else {
            return Err("Account not found".into());
        };
        if accounts[index].standing == Some(Standing::Deactivated) {
            accounts[index].standing = Some(Standing::Deleted);
            kept(&mut store, index);
            return self.save(&store);
        }
        accounts.remove(index);
        if P::current(&mut store).as_deref() == Some(id) {
            *P::current(&mut store) = None;
        }
        self.save(&store)
    }
    pub(crate) async fn edit(
        &self,
        edit: impl FnOnce(&mut [&mut dyn PoolAccount]) -> Result<bool>,
    ) -> Result<()> {
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        let changed = {
            let mut accounts: Vec<&mut dyn PoolAccount> = P::accounts_mut(&mut store)
                .iter_mut()
                .map(|a| a as &mut dyn PoolAccount)
                .collect();
            edit(&mut accounts)?
        };
        if changed {
            self.save(&store)?;
        }
        Ok(())
    }
    /// Whether the account failed: its usage refused, or its token expired.
    pub(crate) fn login_failed(&self, id: &str) -> Result<bool> {
        let store = self.load()?;
        let account = P::accounts(&store)
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?;
        Ok(account.usage_error.is_some()
            || P::expiry(&account.document).is_some_and(|e| e <= now_unix_seconds()))
    }

    /// Gives the tool account `id`'s login, as read before (`live`, which
    /// the caller reads under the operations lock it holds). A Live login
    /// the pool does not hold is kept in the data directory's `backup`
    /// first. `leave` takes the tool off a provider, under the store's lock:
    /// what it changes in the store is not kept; `left` records that the
    /// switch is made, and `undo` puts back what `leave` changed elsewhere
    /// should the write fail. Returns the account switched to.
    pub(crate) async fn switch<L>(
        &self,
        id: &str,
        live: Option<P::Login>,
        access: KeychainAccess,
        backup: &str,
        leave: impl FnOnce(&mut P::Store) -> Result<L>,
        undo: impl FnOnce(L),
        left: impl FnOnce(&mut P::Store),
    ) -> Result<Account<P::Own>> {
        let locks = self.0.locks();
        // The store is not locked while the login is written, which may
        // wait for the user to allow it: the cards are read meanwhile.
        let (target, prepared) = {
            let _lock = locks.store.lock().await;
            let mut store = self.load()?;
            if Self::follow_live(&mut store, live.as_ref()) {
                self.save(&store)?;
            }
            let target = P::accounts(&store)
                .iter()
                .find(|a| a.id == id)
                .ok_or("Account not found")?
                .clone();
            if target.standing.is_some() {
                return Err("Account is deactivated or hidden".into());
            }
            if P::access_token(&target.document).is_none() {
                return Err("The account holds no tokens".into());
            }
            if let Some(live) = live.as_ref().filter(|_| P::current(&mut store).is_none()) {
                write_json(&self.0.app().paths.data_dir.join(backup), P::document(live))?;
            }
            (target, leave(&mut store)?)
        };
        if let Err(error) = self.0.write_live(&P::login_of(&target), access).await {
            undo(prepared);
            return Err(error);
        }
        let _lock = locks.store.lock().await;
        let mut store = self.load()?;
        left(&mut store);
        // Unless the account was removed while its login was written.
        let kept = P::accounts(&store).iter().any(|a| a.id == target.id);
        *P::current(&mut store) = kept.then(|| target.id.clone());
        locks.switches.fetch_add(1, Ordering::SeqCst);
        self.save(&store)?;
        Ok(target)
    }

    /// A refresh's start, under the operations lock the caller holds: the
    /// account, once the Live login is followed.
    pub(crate) async fn begin(&self, id: &str) -> Result<Begun<P::Own>> {
        let live = self.0.read_live(KeychainAccess::Background).await;
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        if Self::follow_live(&mut store, live.as_ref().ok().and_then(Option::as_ref)) {
            self.save(&store)?;
        }
        let current = P::current(&mut store).as_deref() == Some(id);
        let account = P::accounts(&store)
            .iter()
            .find(|a| a.id == id)
            .ok_or("Account not found")?
            .clone();
        Ok(Begun {
            account,
            live_unreadable: current && live.is_err(),
        })
    }

    /// The Live login's document when it holds the account's login with
    /// another access token, unexpired: the tool renewed it.
    async fn live_renewal(
        &self,
        account: &Account<P::Own>,
        document: &Value,
        now: i64,
    ) -> Option<Value> {
        let live = self
            .0
            .read_live(KeychainAccess::Background)
            .await
            .ok()
            .flatten()?;
        let live = P::document(&live);
        let renewed = P::same_account(account, live)
            && P::access_token(live).is_some()
            && P::access_token(live) != P::access_token(document)
            && P::expiry(live).is_none_or(|e| e > now + EXPIRY_MARGIN);
        renewed.then(|| live.clone())
    }
    /// Renews the document's access token, then gives the renewal to the
    /// Live login when it holds this login: an issuer that rotates refresh
    /// tokens stops honoring the one the tool holds. Not while the Live
    /// login the account may be cannot be read. All of it under the tool's
    /// renewal lock, if any; a later generation the tool renewed while the
    /// lock was awaited serves instead.
    async fn renew(
        &self,
        account: &Account<P::Own>,
        document: &mut Value,
        proxy: &str,
        live_unreadable: bool,
        now: i64,
    ) -> Result<Renewing> {
        if live_unreadable {
            return Err(P::LIVE_UNREADABLE.into());
        }
        let lock = self.0.renewal_lock().await?;
        if let Some(live) = self
            .live_renewal(account, document, now)
            .await
            .filter(|live| P::newer(live, document))
        {
            *document = live;
            return Ok(Renewing::Adopted);
        }
        if let Some(lock) = &lock {
            lock.held()?;
        }
        let before = document.clone();
        let renewed = self.0.renew(document, proxy).await?;
        self.write_back(&renewed, &before, document).await;
        drop(lock);
        Ok(Renewing::Renewed)
    }
    /// Puts a renewal in the Live login, in its own form, when it still
    /// holds the login renewed (`before`, with the same refresh token) and
    /// no later generation than the renewal (`after`); never another login
    /// or a newer token.
    /// Silent: no authorization UI, and a failure only logged, for the
    /// store keeps the renewal.
    async fn write_back(&self, renewed: &P::Renewed, before: &Value, after: &Value) {
        let access = P::access_token(after);
        let outcome: Result<()> = async {
            let Some(mut live) = self.0.read_live(KeychainAccess::Background).await? else {
                return Ok(());
            };
            let document = P::document(&live);
            // Still the generation renewed, or one the renewal outlasts.
            let holds_before = P::access_token(document) == P::access_token(before);
            if !P::same_login(document, before)
                // The tool renewed it from another refresh token meanwhile.
                || P::refresh_token(document) != P::refresh_token(before)
                || !holds_before && P::newer(document, after)
                || P::access_token(document) == access
            {
                return Ok(());
            }
            P::apply(renewed, P::document_mut(&mut live));
            self.0.write_live(&live, KeychainAccess::Background).await
        }
        .await;
        if let Err(error) = outcome {
            log::warn!(
                "{}'s login was not given the renewal; Copool keeps it: {error}",
                P::NAME
            );
        }
    }

    /// Asks with the account's login, renewing its token as its renewal
    /// allows; `forced` only renews it, whatever the choice. A token expired
    /// or refused gives way first to the one the tool renewed, if any; a
    /// refused one is renewed once.
    pub(crate) async fn use_token<T, Ask, Asking>(
        &self,
        begun: &Begun<P::Own>,
        forced: bool,
        proxy: &str,
        now: i64,
        ask: Ask,
    ) -> Used<T>
    where
        Ask: Fn(Value) -> Asking,
        Asking: Future<Output = Asked<T>>,
    {
        let account = &begun.account;
        let unreadable = begun.live_unreadable;
        let mut document = account.document.clone();
        let may_renew = forced || Renewal::of(account.renewal) != Renewal::Never;
        let mut renewed = false;
        let mut adopted = false;
        let outcome: Result<Option<T>> = async {
            if forced {
                match self
                    .renew(account, &mut document, proxy, unreadable, now)
                    .await?
                {
                    Renewing::Renewed => renewed = true,
                    Renewing::Adopted => adopted = true,
                }
                return Ok(None);
            }
            let expired = P::expiry(&document).is_some_and(|e| e <= now + EXPIRY_MARGIN)
                || P::access_token(&document).is_none();
            if expired {
                if let Some(live) = self.live_renewal(account, &document, now).await {
                    document = live;
                    adopted = true;
                } else {
                    if !may_renew {
                        return Err(RENEWAL_OFF.into());
                    }
                    match self
                        .renew(account, &mut document, proxy, unreadable, now)
                        .await?
                    {
                        Renewing::Renewed => renewed = true,
                        Renewing::Adopted => adopted = true,
                    }
                }
            }
            match ask(document.clone()).await {
                Asked::Answered(answer) => return Ok(Some(answer)),
                Asked::Failed(error) => return Err(error),
                Asked::Refused if renewed => return Err(P::SIGN_IN_EXPIRED.into()),
                Asked::Refused => {}
            }
            // The tool may have renewed the login since it was read: its
            // token is tried once before renewing.
            if let Some(live) = self.live_renewal(account, &document, now).await {
                document = live;
                adopted = true;
                match ask(document.clone()).await {
                    Asked::Answered(answer) => return Ok(Some(answer)),
                    Asked::Failed(error) => return Err(error),
                    Asked::Refused => {}
                }
            }
            if !may_renew {
                return Err(RENEWAL_OFF.into());
            }
            match self
                .renew(account, &mut document, proxy, unreadable, now)
                .await?
            {
                Renewing::Renewed => renewed = true,
                Renewing::Adopted => adopted = true,
            }
            match ask(document.clone()).await {
                Asked::Answered(answer) => Ok(Some(answer)),
                Asked::Refused => Err(P::SIGN_IN_EXPIRED.into()),
                Asked::Failed(error) => Err(error),
            }
        }
        .await;
        Used {
            outcome,
            document,
            renewed,
            adopted,
        }
    }

    /// Keeps a refresh's login in the account, if it is still kept, then
    /// lets `update` record the rest. The tool may have renewed the login
    /// meanwhile: the newer stays. A login taken from the tool replaces the
    /// card's whatever its expiry, for its refresh token is the one that
    /// still works, unless the card changed meanwhile. Returns whether the
    /// account was still kept.
    pub(crate) async fn record<T>(
        &self,
        begun: &Begun<P::Own>,
        used: &Used<T>,
        update: impl FnOnce(&mut P::Store, usize),
    ) -> Result<bool> {
        let _lock = self.0.locks().store.lock().await;
        let mut store = self.load()?;
        let Some(index) = P::accounts(&store)
            .iter()
            .position(|a| a.id == begun.account.id)
        else {
            return Ok(false);
        };
        let stored = &mut P::accounts_mut(&mut store)[index];
        if used.renewed
            || (used.adopted && stored.document == begun.account.document)
            || P::newer(&used.document, &stored.document)
        {
            stored.document = used.document.clone();
        }
        update(&mut store, index);
        self.save(&store)?;
        Ok(true)
    }
}

/// What each pool is tested for alike, through its own fixture: its real
/// facts, with its tool's Live login and its issuer in memory or a
/// temporary directory. [`login_store_suite`] runs the suite for a fixture.
#[cfg(test)]
pub(crate) mod suite {
    use crate::{application::Application, pool::Pool, utils::now_unix_seconds};
    use serde_json::{json, Value};
    use std::future::Future;

    /// A pool's application and its tool and issuer, as tests drive them.
    /// Its issuer refuses access tokens starting with "expired".
    pub(crate) trait Fixture: Sized + Sync {
        const POOL: Pool;
        /// The error of an account not renewed while the Live login cannot
        /// be read.
        const LIVE_UNREADABLE: &'static str;
        fn open(request_new_accounts: bool) -> impl Future<Output = Self>;
        fn app(&self) -> &Application;
        /// A login of `user` with this access token, its refresh token
        /// `refresh-<user>`.
        fn login(user: &str, access: &str, expires: i64) -> Value;
        /// The login with another refresh token, as the tool's own renewal
        /// rotates it.
        fn with_refresh(login: &Value, refresh: &str) -> Value;
        /// The login without its expiry.
        fn without_expiry(login: &Value) -> Value;
        /// A login without a refresh token, which no import takes.
        fn no_login() -> Value;
        fn access(document: &Value) -> Option<String>;
        /// The access token a renewal of `user`'s login hands out.
        fn renewed(user: &str) -> String;
        /// The tool signs in with `login`.
        fn sign_in(&self, login: &Value);
        fn live(&self) -> Value;
        /// The tool's Live login can no longer be read.
        fn corrupt_live(&self);
        /// Reads of the Live login answer this late with what was there
        /// when asked.
        fn delay_live_reads(&self, ms: u64);
        /// The tool signs in with `login` while the next renewal is asked.
        fn meanwhile(&self, login: Value);
        fn renewals(&self) -> usize;
        /// `user`'s account.
        fn id_of(&self, user: &str) -> String;
        fn stored(&self, id: &str) -> Value;
        /// The account current as the store last saw it.
        fn current(&self) -> Option<String>;
    }

    fn later() -> i64 {
        now_unix_seconds() + 3600
    }
    fn import<F: Fixture>(
        f: &F,
        logins: Value,
    ) -> impl Future<Output = Result<usize, String>> + '_ {
        let content = logins.to_string();
        async move { f.app().import_json(F::POOL, &content).await }
    }

    pub(crate) async fn imports_are_checked_whole_and_folded_into_known_accounts<F: Fixture>() {
        let f = F::open(true).await;
        let alice = F::login("alice", "access-alice", later());
        assert_eq!(import(&f, json!([alice])).await.unwrap(), 1);
        // One login without a refresh token refuses the whole import.
        let bob = F::login("bob", "access-bob", later());
        assert!(import(&f, json!([bob, F::no_login()])).await.is_err());
        assert_eq!(f.app().accounts(F::POOL).await.unwrap().len(), 1);
        // A newer generation of the same login updates its account.
        let newer = F::login("alice", "access-alice-2", later() + 60);
        assert_eq!(import(&f, newer).await.unwrap(), 1);
        let cards = f.app().accounts(F::POOL).await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(
            F::access(&f.stored(&cards[0].id)).as_deref(),
            Some("access-alice-2")
        );
    }

    pub(crate) async fn a_browser_sign_in_joins_the_pool_without_switching<F: Fixture>() {
        let f = F::open(true).await;
        f.sign_in(&F::login("alice", "access-alice", later()));
        f.app().import_current(F::POOL).await.unwrap();
        let bob = F::login("bob", "access-bob", later());
        f.app().import_sign_in(F::POOL, bob, None).await.unwrap();
        let cards = f.app().accounts(F::POOL).await.unwrap();
        assert_eq!(cards.len(), 2);
        let current: Vec<_> = cards
            .iter()
            .filter(|c| c.is_current)
            .map(|c| &c.id)
            .collect();
        assert_eq!(current, [&f.id_of("alice")]);
        assert_eq!(F::access(&f.live()).as_deref(), Some("access-alice"));
    }

    pub(crate) async fn cards_read_before_a_switch_keep_the_account_switched_to<F: Fixture>() {
        let f = F::open(true).await;
        let (alice, bob) = (
            F::login("alice", "access-alice", later()),
            F::login("bob", "access-bob", later()),
        );
        import(&f, json!([alice, bob])).await.unwrap();
        f.sign_in(&alice);
        f.app().accounts(F::POOL).await.unwrap();
        let bob = f.id_of("bob");
        // The cards read Alice's login, which a switch to Bob replaces
        // before the read answers.
        f.delay_live_reads(300);
        let (_, switched) = tokio::join!(f.app().accounts(F::POOL), async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            f.delay_live_reads(0);
            f.app().switch_account(F::POOL, &bob).await
        });
        switched.unwrap();
        assert_eq!(f.current(), Some(bob));
    }

    pub(crate) async fn the_live_login_shares_its_renewal_and_another_cards_leaves_it_alone<
        F: Fixture,
    >() {
        let f = F::open(true).await;
        f.sign_in(&F::login("alice", "expired-alice", now_unix_seconds() - 10));
        // Imported, Alice's expired token is renewed, and the tool's with it:
        // an issuer rotating refresh tokens would not honor the tool's.
        f.app().import_current(F::POOL).await.unwrap();
        let alice = f.id_of("alice");
        assert_eq!(F::access(&f.stored(&alice)), Some(F::renewed("alice")));
        assert_eq!(F::access(&f.live()), Some(F::renewed("alice")));
        // A refused token of an account the tool does not use is renewed;
        // the tool's login stays as it was.
        let bob = F::login("bob", "expired-bob", later());
        f.app().import_sign_in(F::POOL, bob, None).await.unwrap();
        assert_eq!(
            F::access(&f.stored(&f.id_of("bob"))),
            Some(F::renewed("bob"))
        );
        assert_eq!(F::access(&f.live()), Some(F::renewed("alice")));
        assert_eq!(f.renewals(), 2);
    }

    pub(crate) async fn an_expired_token_takes_the_one_the_tool_renewed<F: Fixture>() {
        let f = F::open(false).await;
        let expired = F::login("alice", "access-alice", now_unix_seconds() - 10);
        f.app()
            .import_sign_in(F::POOL, expired, None)
            .await
            .unwrap();
        let id = f.id_of("alice");
        // The tool renewed the login since, its expiry unknown; renewal off
        // does not keep the card from taking it.
        let live = F::without_expiry(&F::login("alice", "live-alice", later()));
        f.sign_in(&live);
        f.app().set_renewal(F::POOL, &id, "never").await.unwrap();
        f.app()
            .release_new_accounts(F::POOL, Some(&id))
            .await
            .unwrap();
        f.app().refresh_usage(F::POOL, &id).await.unwrap();
        assert_eq!(F::access(&f.stored(&id)).as_deref(), Some("live-alice"));
        assert_eq!(f.renewals(), 0);
    }

    pub(crate) async fn a_refused_token_gives_way_to_the_one_the_tool_renewed<F: Fixture>() {
        let f = F::open(false).await;
        let refused = F::login("alice", "expired-alice", later());
        f.app()
            .import_sign_in(F::POOL, refused, None)
            .await
            .unwrap();
        let id = f.id_of("alice");
        f.sign_in(&F::without_expiry(&F::login(
            "alice",
            "live-alice",
            later(),
        )));
        f.app()
            .release_new_accounts(F::POOL, Some(&id))
            .await
            .unwrap();
        f.app().refresh_usage(F::POOL, &id).await.unwrap();
        assert_eq!(F::access(&f.stored(&id)).as_deref(), Some("live-alice"));
        assert_eq!(f.renewals(), 0);
    }

    pub(crate) async fn a_renewal_never_replaces_a_login_the_tool_renewed_meanwhile<F: Fixture>() {
        let f = F::open(true).await;
        f.sign_in(&F::login("alice", "access-alice", later()));
        f.app().import_current(F::POOL).await.unwrap();
        let id = f.id_of("alice");
        // The tool renews from its refresh token first; Copool's renewal,
        // from the one it held, leaves that login alone.
        let theirs = F::with_refresh(
            &F::login("alice", "tool-renewed", later() + 7200),
            "tool-rotated",
        );
        f.meanwhile(theirs.clone());
        f.app().renew_now(F::POOL, &id).await.unwrap();
        assert_eq!(f.live(), theirs);
        assert_eq!(F::access(&f.stored(&id)), Some(F::renewed("alice")));
    }

    pub(crate) async fn a_renewal_never_replaces_a_token_the_tool_renewed_for_longer<F: Fixture>() {
        let f = F::open(true).await;
        f.sign_in(&F::login("alice", "expired-alice", now_unix_seconds() - 10));
        // The tool renews the same login, for longer, while Copool renews it.
        let renewed = F::login("alice", "tool-renewed", now_unix_seconds() + 86_400);
        f.meanwhile(renewed.clone());
        f.app().import_current(F::POOL).await.unwrap();
        assert_eq!(f.renewals(), 1);
        assert_eq!(f.live(), renewed);
        // The pool follows the newer token on its next read.
        f.app().accounts(F::POOL).await.unwrap();
        assert_eq!(
            F::access(&f.stored(&f.id_of("alice"))).as_deref(),
            Some("tool-renewed")
        );
    }

    pub(crate) async fn the_account_in_use_is_not_renewed_while_its_live_login_cannot_be_read<
        F: Fixture,
    >() {
        let f = F::open(false).await;
        f.sign_in(&F::login("alice", "expired-alice", later()));
        f.app().import_current(F::POOL).await.unwrap();
        let bob = F::login("bob", "expired-bob", later());
        f.app().import_sign_in(F::POOL, bob, None).await.unwrap();
        let (alice, bob) = (f.id_of("alice"), f.id_of("bob"));
        f.app().release_new_accounts(F::POOL, None).await.unwrap();
        // Renewing the account the tool uses would rotate the refresh token
        // its login holds, which could not be given the new one.
        f.corrupt_live();
        assert_eq!(
            f.app().refresh_usage(F::POOL, &alice).await.unwrap_err(),
            F::LIVE_UNREADABLE
        );
        assert_eq!(f.renewals(), 0);
        // Another account is renewed as ever.
        f.app().refresh_usage(F::POOL, &bob).await.unwrap();
        assert_eq!(f.renewals(), 1);
    }
}

/// The suite of [`suite`], as a pool's tests run it with their fixture.
#[cfg(test)]
macro_rules! login_store_suite {
    ($fixture:ty, $($test:ident),+ $(,)?) => {
        mod login_store_suite {
            $(
                #[tokio::test]
                async fn $test() {
                    crate::login_store::suite::$test::<$fixture>().await
                }
            )+
        }
    };
    ($fixture:ty) => {
        crate::login_store::login_store_suite!(
            $fixture,
            imports_are_checked_whole_and_folded_into_known_accounts,
            a_browser_sign_in_joins_the_pool_without_switching,
            cards_read_before_a_switch_keep_the_account_switched_to,
            the_live_login_shares_its_renewal_and_another_cards_leaves_it_alone,
            an_expired_token_takes_the_one_the_tool_renewed,
            a_refused_token_gives_way_to_the_one_the_tool_renewed,
            a_renewal_never_replaces_a_login_the_tool_renewed_meanwhile,
            a_renewal_never_replaces_a_token_the_tool_renewed_for_longer,
            the_account_in_use_is_not_renewed_while_its_live_login_cannot_be_read,
        );
    };
}
#[cfg(test)]
pub(crate) use login_store_suite;
