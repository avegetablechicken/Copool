use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
    time::Duration,
};
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Options {
    pub(crate) timeout: Duration,
    pub(crate) user_agent: String,
    pub(crate) insecure: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(18),
            user_agent: "codex-tools/0.1".into(),
            insecure: false,
        }
    }
}
pub(crate) struct Session {
    id: u64,
    pub(crate) client: reqwest::Client,
    options: Options,
    pub(crate) proxy_url: Option<String>,
    pub(crate) allow_failover: bool,
}
impl Session {
    pub(crate) fn new(options: Options, proxy_url: Option<String>) -> Result<Arc<Self>, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let builder = reqwest::Client::builder()
            .timeout(options.timeout)
            .user_agent(&options.user_agent)
            .danger_accept_invalid_certs(options.insecure);
        let client =
            crate::utils::with_account_proxy(builder, proxy_url.as_deref().unwrap_or_default())?
                .build()
                .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            client,
            options,
            proxy_url,
            allow_failover: false,
        }))
    }
}
#[derive(Default)]
pub(crate) struct Sessions {
    cache: Mutex<HashMap<(u64, String), Arc<Session>>>,
}
impl Sessions {
    pub(crate) fn session(
        &self,
        proxy: &str,
        fallback: &Arc<Session>,
    ) -> Result<Arc<Session>, String> {
        if crate::utils::validate_account_proxy(proxy)?.is_none() {
            return Ok(fallback.clone());
        }
        let key = (fallback.id, proxy.trim().to_owned());
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "Proxy session cache unavailable")?;
        if let Some(session) = cache.get(&key) {
            return Ok(session.clone());
        }
        let session = Session::new(fallback.options.clone(), Some(proxy.trim().into()))?;
        if cache.len() >= 32 {
            if let Some(old) = cache.keys().next().cloned() {
                cache.remove(&old);
            }
        }
        cache.insert(key, session.clone());
        Ok(session)
    }
}
pub(crate) fn client(proxy: &str, options: Options) -> Result<reqwest::Client, String> {
    static FALLBACKS: LazyLock<Mutex<HashMap<Options, Arc<Session>>>> =
        LazyLock::new(Default::default);
    static POOL: LazyLock<Sessions> = LazyLock::new(Default::default);
    let fallback = {
        let mut cache = FALLBACKS
            .lock()
            .map_err(|_| "HTTP client cache unavailable")?;
        if let Some(client) = cache.get(&options) {
            client.clone()
        } else {
            let session = Session::new(options.clone(), None)?;
            cache.insert(options, session.clone());
            session
        }
    };
    Ok(POOL.session(proxy, &fallback)?.client.clone())
}
