//! Persistent, per-account greetings. Timers belong to the service, not the webview.
use crate::{persistence::write_json, AccountSummary, Application};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GreetingSchedule {
    pub auto: bool,
    pub times: Vec<String>,
    pub last_error: Option<String>,
    last_auto: Option<i64>,
    retry_after: i64,
    last_times: BTreeMap<String, String>,
}
type Schedules = BTreeMap<String, GreetingSchedule>;
fn key(claude: bool, id: &str) -> String {
    format!("{}:{id}", if claude { "claude" } else { "codex" })
}
fn normalize_times(times: &mut Vec<String>) -> Result<()> {
    if times.len() > 48 {
        return Err("At most 48 greeting times are allowed".into());
    }
    for time in times.iter() {
        let b = time.as_bytes();
        if b.len() != 5
            || b[2] != b':'
            || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit)
            || &time[..2] > "23"
            || &time[3..] > "59"
        {
            return Err("Invalid greeting time; use HH:MM".into());
        }
    }
    times.sort();
    times.dedup();
    Ok(())
}
fn eligible(account: &AccountSummary, claude: bool) -> bool {
    !account.awaiting_first_request
        && (claude
            || (account.source_tag.as_deref() != Some("SUB2API")
                && account
                    .usage
                    .as_ref()
                    .and_then(|u| u.plan_type.as_deref())
                    .filter(|s| !s.trim().is_empty())
                    .or(account.plan_type.as_deref())
                    .is_some_and(|p| p.trim().eq_ignore_ascii_case("plus"))))
}
impl GreetingSchedule {
    fn due(
        &self,
        account: &AccountSummary,
        now: i64,
        day: &str,
        time: &str,
    ) -> (Option<i64>, bool) {
        let reset = account
            .usage
            .as_ref()
            .map(|u| u.five_hour.as_ref().and_then(|w| w.reset_at).unwrap_or(0));
        let offset = account
            .usage
            .as_ref()
            .and_then(|u| u.five_hour.as_ref())
            .and_then(|w| w.clock_offset_seconds)
            .unwrap_or(0);
        let auto = reset.filter(|reset| {
            self.auto
                && *reset <= now.saturating_add(offset)
                && self.last_auto.is_none_or(|last| *reset > last)
                && now >= self.retry_after
        });
        let timed = self
            .times
            .iter()
            .any(|t| t == time && self.last_times.get(t).is_none_or(|d| d != day));
        (auto, timed)
    }
}
pub(crate) struct GreetingGuard<'a> {
    app: &'a Application,
    key: String,
}
impl Drop for GreetingGuard<'_> {
    fn drop(&mut self) {
        self.app
            .greetings_inflight
            .lock()
            .unwrap()
            .remove(&self.key);
    }
}
impl Application {
    pub(crate) fn begin_greeting(&self, claude: bool, id: &str) -> Result<GreetingGuard<'_>> {
        let key = key(claude, id);
        if !self.greetings_inflight.lock().unwrap().insert(key.clone()) {
            return Err("A greeting is already in progress for this account".into());
        }
        Ok(GreetingGuard { app: self, key })
    }
    fn load_greetings(&self) -> Result<Schedules> {
        match std::fs::read(self.paths.data_dir.join("greetings.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn save_greetings(&self, schedules: &Schedules) -> Result<()> {
        write_json(&self.paths.data_dir.join("greetings.json"), schedules)
    }
    pub async fn greeting_schedule(&self, claude: bool, id: &str) -> Result<GreetingSchedule> {
        let _lock = self.operations.lock().await;
        Ok(self
            .load_greetings()?
            .remove(&key(claude, id))
            .unwrap_or_default())
    }
    pub async fn set_greeting_schedule(
        &self,
        claude: bool,
        id: &str,
        auto: bool,
        mut times: Vec<String>,
    ) -> Result<()> {
        normalize_times(&mut times)?;
        let accounts = if claude {
            self.claude_accounts().await?
        } else {
            self.accounts().await?
        };
        if !accounts.iter().any(|a| a.id == id && eligible(a, claude)) {
            return Err("Account is not eligible for scheduled greetings; refresh it first".into());
        }
        let _lock = self.operations.lock().await;
        let mut all = self.load_greetings()?;
        let schedule = all.entry(key(claude, id)).or_default();
        schedule.auto = auto;
        schedule.times = times;
        self.save_greetings(&all)
    }
    pub(crate) async fn background_greetings(self: Arc<Self>) {
        let mut tasks = tokio::task::JoinSet::new();
        let mut running = std::collections::BTreeSet::new();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Ok(key) = result { running.remove(&key); }
                }
                _ = interval.tick() => {
                    let enabled = {
                        let _lock = self.operations.lock().await;
                        self.load_greetings().unwrap_or_default()
                    };
                    for claude in [false, true] {
                        let prefix = if claude { "claude:" } else { "codex:" };
                        if !enabled.iter().any(|(key, s)| key.starts_with(prefix) && (s.auto || !s.times.is_empty())) { continue; }
                        let accounts = if claude { self.claude_accounts().await } else { self.accounts().await };
                        let Ok(accounts) = accounts else { continue };
                        let now = Local::now();
                        let day = now.format("%Y-%m-%d").to_string();
                        let time = now.format("%H:%M").to_string();
                        let _lock = self.operations.lock().await;
                        let Ok(mut all) = self.load_greetings() else { continue };
                        for account in accounts.iter().filter(|a| eligible(a, claude)) {
                            let key = key(claude, &account.id);
                            if running.contains(&key) { continue; }
                            let Some(schedule) = all.get_mut(&key) else { continue };
                            let (auto, timed) = schedule.due(account, now.timestamp(), &day, &time);
                            if auto.is_none() && !timed { continue; }
                            // Persist the claim before sending: restarts cannot replay a daily time.
                            schedule.retry_after = now.timestamp() + 300;
                            if timed { schedule.last_times.insert(time.clone(), day.clone()); }
                            if self.save_greetings(&all).is_err() { continue; }
                            running.insert(key.clone());
                            let app = self.clone();
                            let id = account.id.clone();
                            tasks.spawn(async move {
                                let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
                                    if claude { app.claude_say_hi(&id).await } else { app.say_hi(&id).await }
                                }).await.unwrap_or_else(|_| Err("Scheduled greeting timed out".into()));
                                {
                                    let _lock = app.operations.lock().await;
                                    if let Ok(mut all) = app.load_greetings() {
                                        if let Some(schedule) = all.get_mut(&key) {
                                            schedule.last_error = result.clone().err();
                                            if result.is_ok() && auto.is_some() { schedule.last_auto = auto; }
                                            let _ = app.save_greetings(&all);
                                        }
                                    }
                                }
                                // Usage can lag a greeting by minutes. Refresh without sending another hi.
                                if result.is_ok() {
                                    tokio::spawn(async move {
                                    for delay in [0, 10, 20, 60, 90] {
                                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                                        let _ = if claude { app.claude_refresh_usage(&id).await } else { app.refresh_usage(&id).await };
                                        let cards = if claude { app.claude_accounts().await } else { app.accounts().await };
                                        if cards.is_ok_and(|cards| cards.iter().any(|a| a.id == id && a.usage.as_ref()
                                            .and_then(|u| u.five_hour.as_ref()).and_then(|w| w.reset_at)
                                            .is_some_and(|r| r > chrono::Utc::now().timestamp()))) { break; }
                                    }
                                    });
                                }
                                key
                            });
                        }
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_validation_and_deduplication() {
        let mut times = vec!["23:59".into(), "08:00".into(), "08:00".into()];
        normalize_times(&mut times).unwrap();
        assert_eq!(times, ["08:00", "23:59"]);
        for bad in ["24:00", "12:60", "8:00", "aa:bb", "😀x"] {
            assert!(normalize_times(&mut vec![bad.into()]).is_err());
        }
    }
    #[test]
    fn auto_waits_for_reset_and_daily_times_are_not_replayed() {
        let stored: crate::models::StoredAccount = serde_json::from_value(serde_json::json!({
            "id":"test", "label":"test", "accountId":"test", "addedAt":0, "updatedAt":0, "authJson":{}
        })).unwrap();
        let mut account = stored.to_summary(None);
        let mut s = GreetingSchedule {
            auto: true,
            times: vec!["08:00".into()],
            ..Default::default()
        };
        assert_eq!(s.due(&account, 100, "2026-01-01", "07:00"), (None, false));
        account.usage = Some(
            serde_json::from_value(
                serde_json::json!({"fiveHour":{"usedPercent":0, "windowSeconds":18000, "resetAt":101}, "fetchedAt":0}),
            )
            .unwrap(),
        );
        assert_eq!(s.due(&account, 100, "2026-01-01", "08:00"), (None, true));
        assert_eq!(
            s.due(&account, 101, "2026-01-01", "08:00"),
            (Some(101), true)
        );
        account
            .usage
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .clock_offset_seconds = Some(10);
        assert_eq!(
            s.due(&account, 91, "2026-01-01", "07:00"),
            (Some(101), false)
        );
        account
            .usage
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .clock_offset_seconds = None;
        s.last_auto = Some(101);
        s.last_times.insert("08:00".into(), "2026-01-01".into());
        assert_eq!(s.due(&account, 102, "2026-01-01", "08:00"), (None, false));
        assert_eq!(s.due(&account, 102, "2026-01-02", "08:00"), (None, true));
        s.last_auto = None;
        s.retry_after = 200;
        assert_eq!(s.due(&account, 102, "2026-01-01", "08:00"), (None, false));
    }
    struct NoSecrets;
    impl crate::credentials::SecretStore for NoSecrets {
        fn get(&self, _: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn remove(&self, _: &str) -> Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn schedules_survive_restart_and_concurrent_greetings_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::Paths {
            data_dir: dir.path().join("data"),
            codex_auth: dir.path().join("codex/auth.json"),
        };
        let app = Application::open_with_secrets(paths.clone(), Arc::new(NoSecrets)).unwrap();
        write_json(
            &app.paths.store(),
            &serde_json::json!({"accounts":[{
                "id":"a", "label":"test", "accountId":"a", "planType":"plus",
                "addedAt":0, "updatedAt":0, "authJson":{}
            }]}),
        )
        .unwrap();
        app.set_greeting_schedule(false, "a", true, vec!["09:00".into(), "08:00".into()])
            .await
            .unwrap();
        let guard = app.begin_greeting(false, "a").unwrap();
        assert!(app.begin_greeting(false, "a").is_err());
        assert!(app.begin_greeting(true, "a").is_ok());
        drop(guard);
        assert!(app.begin_greeting(false, "a").is_ok());
        drop(app);
        let app = Application::open_with_secrets(paths, Arc::new(NoSecrets)).unwrap();
        let schedule = app.greeting_schedule(false, "a").await.unwrap();
        assert!(schedule.auto);
        assert_eq!(schedule.times, ["08:00", "09:00"]);
        assert!(!app.greeting_schedule(true, "a").await.unwrap().auto);
        app.set_greeting_schedule(false, "a", false, vec![])
            .await
            .unwrap();
        assert!(!app.greeting_schedule(false, "a").await.unwrap().auto);
    }
}
