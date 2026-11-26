#[path = "../../src/antigravity_cloud.rs"]
mod antigravity_cloud;
#[path = "../../src/proxy_session.rs"]
mod proxy_session;
#[path = "../../src/settings.rs"]
mod settings;
#[path = "../../src/repository.rs"]
mod repository;
#[path = "../../src/endpoint.rs"]
mod endpoint;
#[path = "../../src/auth.rs"]
mod auth;
#[path = "../../src/claude_api.rs"]
mod claude_api;
#[path = "../../src/models.rs"]
mod models;
#[path = "../../src/pool_login/mod.rs"]
mod pool_login;
#[path = "../../src/proxy_daemon.rs"]
mod proxy_daemon;
#[path = "../../src/proxy_service.rs"]
mod proxy_service;
#[path = "../../src/state.rs"]
mod state;
#[path = "../../src/store.rs"]
mod store;
#[path = "../../src/usage.rs"]
mod usage;
#[path = "../../src/utils.rs"]
mod utils;

fn main() {
    if let Err(error) = proxy_daemon::run_cli_from_env() {
        eprintln!("{}", utils::unlocalized(&error));
        std::process::exit(1);
    }
}
