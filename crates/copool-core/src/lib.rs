//! Platform-independent Copool application services.
//! The packaged remote daemon and desktop share one proxy engine source tree.
#![allow(dead_code)]
#[path = "../../../resources/proxyd-src/src/antigravity_cloud.rs"]
mod antigravity_cloud;
mod application;
#[path = "../../../resources/proxyd-src/src/auth.rs"]
mod auth;
#[path = "../../../resources/proxyd-src/src/claude_api.rs"]
mod claude_api;
#[path = "../../../resources/proxyd-src/src/models.rs"]
mod models;
mod persistence;
#[path = "../../../resources/proxyd-src/src/pool_login/mod.rs"]
mod pool_login;
#[path = "../../../resources/proxyd-src/src/proxy_service.rs"]
mod proxy_service;
#[path = "../../../resources/proxyd-src/src/state.rs"]
mod state;
#[path = "../../../resources/proxyd-src/src/store.rs"]
mod store;
#[path = "../../../resources/proxyd-src/src/usage.rs"]
mod usage;
#[path = "../../../resources/proxyd-src/src/utils.rs"]
mod utils;
pub use application::{
    AgentSync, AgentSyncOutcome, Application, Paths, Preferences, SmartSwitchOutcome,
    SwitchExecution,
};
pub use models::{AccountSummary, ApiProxyStatus, CreditSnapshot, UsageSnapshot, UsageWindow};
pub mod codex;
mod login_store;
mod plan_tags;
mod pool;
pub use pool::Pool;

#[cfg(test)]
mod legacy_tests;
mod ranking;
mod system_proxy;

mod refresh_plan;
mod refresh_policy;
pub use refresh_policy::{PoolRefresh, RefreshPreferences};

#[path = "../../../resources/proxyd-src/src/endpoint.rs"]
mod endpoint;

mod remote;

pub mod widgets;

pub mod deployment;
pub mod process;

#[path = "../../../resources/proxyd-src/src/settings.rs"]
pub mod settings;

#[path = "../../../resources/proxyd-src/src/repository.rs"]
mod repository;

#[path = "../../../resources/proxyd-src/src/proxy_session.rs"]
mod proxy_session;

mod cliproxyapi;
pub mod credentials;
pub mod sub2api;
mod vault;

mod provider_accounts;
mod provider_registry;
mod providers;

pub mod platform;

pub mod login;

mod proxy_integration;

mod remote_runtime;

pub mod tunnel;

pub mod claude;

mod antigravity;

mod env_bridge;

mod grok;

mod greetings;
pub use greetings::GreetingSchedule;
