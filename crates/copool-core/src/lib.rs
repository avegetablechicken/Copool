//! Platform-independent Copool application services.
//! The packaged remote daemon and desktop share one proxy engine source tree.
#![allow(dead_code)]
mod application;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/auth.rs"]
mod auth;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/models.rs"]
mod models;
mod persistence;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/proxy_service.rs"]
mod proxy_service;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/state.rs"]
mod state;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/store.rs"]
mod store;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/usage.rs"]
mod usage;
#[path = "../../../Sources/Copool/Resources/proxyd-src/src/utils.rs"]
mod utils;
pub use application::{Application, Paths, Preferences};
pub use models::{AccountSummary, ApiProxyStatus, CreditSnapshot, UsageSnapshot, UsageWindow};

#[cfg(test)]
mod legacy_tests;
mod ranking;

mod refresh_policy;

#[path = "../../../Sources/Copool/Resources/proxyd-src/src/endpoint.rs"]
mod endpoint;

mod remote;

pub mod widgets;

pub mod deployment;
pub mod process;

#[path = "../../../Sources/Copool/Resources/proxyd-src/src/settings.rs"]
pub mod settings;

#[path = "../../../Sources/Copool/Resources/proxyd-src/src/repository.rs"]
mod repository;

#[path = "../../../Sources/Copool/Resources/proxyd-src/src/proxy_session.rs"]
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
