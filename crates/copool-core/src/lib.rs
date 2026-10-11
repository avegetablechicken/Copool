//! Platform-independent Copool application services.
//! The packaged remote daemon and desktop share one proxy engine source tree.
#![allow(dead_code)]
mod application;
#[path = "../../../resources/proxyd-src/src/auth.rs"]
mod auth;
#[path = "../../../resources/proxyd-src/src/models.rs"]
mod models;
mod persistence;
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
pub use application::{Application, Paths, Preferences, SmartSwitchOutcome, SwitchExecution};
pub use models::{AccountSummary, ApiProxyStatus, CreditSnapshot, UsageSnapshot, UsageWindow};

#[cfg(test)]
mod legacy_tests;
mod ranking;
mod system_proxy;

mod refresh_policy;

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

pub mod codex_config;

pub mod workspaces;

pub mod credentials;
pub mod sub2api;

mod providers;

pub mod platform;

pub mod login;

mod proxy_integration;

mod remote_runtime;

pub mod tunnel;

mod workspace_directory;

mod account_network;

mod import_login;

pub mod claude;
