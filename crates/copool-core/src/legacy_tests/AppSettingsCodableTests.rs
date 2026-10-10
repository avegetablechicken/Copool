use crate::settings::*;
use serde_json::json;
#[test]
fn testDecodeSettingsRequiresFullCurrentShape() {
    assert!(serde_json::from_value::<Settings>(json!({"launchAtStartup":true,"launchCodexAfterSwitch":true,"autoSmartSwitch":false,"syncOpencodeOpenaiAuth":false})).is_err());
}
#[test]
fn testDecodeSettingsWithoutUsageProgressDisplayModeDefaultsToUsed() {
    let settings:Settings=serde_json::from_value(json!({"launchAtStartup":true,"launchCodexAfterSwitch":true,"autoSmartSwitch":false,"syncOpencodeOpenaiAuth":false,"localProxyHostAPIOnly":false,"restartEditorsOnSwitch":false,"restartEditorTargets":[],"autoStartApiProxy":false,"proxyConfiguration":{"preferredPortText":"4141","cloudflared":{"enabled":false,"tunnelMode":"quick","useHTTP2":false,"namedHostname":""}},"remoteServers":[],"locale":"en"})).unwrap();
    assert_eq!(settings.usage_progress_display_mode, "used");
    assert_eq!(settings.sub2_api_provider, Providers::default());
}
#[test]
fn testSub2APIProviderConfigurationRoundTripsWithoutPassword() {
    let provider = Provider {
        provider_id: " my ".into(),
        username: " admin@example.com ".into(),
        password: "secret".into(),
        allow_insecure_tls: true,
        imported_account_ids: vec![2, 1, 2],
        ..Default::default()
    };
    let settings = Settings {
        sub2_api_provider: Providers {
            confirmed_provider_ids: vec!["my".into()],
            providers: vec![provider.clone()],
        },
        ..Default::default()
    };
    let encoded = serde_json::to_string(&settings).unwrap();
    let decoded: Settings = serde_json::from_str(&encoded).unwrap();
    let expected = Provider {
        id: provider.id,
        provider_id: "my".into(),
        username: "admin@example.com".into(),
        allow_insecure_tls: true,
        imported_account_ids: vec![1, 2],
        ..Default::default()
    };
    assert_eq!(
        decoded.sub2_api_provider,
        Providers {
            confirmed_provider_ids: vec!["my".into()],
            providers: vec![expected]
        }
    );
    let raw: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert!(raw["sub2APIProvider"]["providers"][0]
        .get("password")
        .is_none());
    assert!(!encoded.contains("secret"));
}
#[test]
fn testLegacySingleConfigurationMigratesWithoutImplicitConfirmation() {
    let config:Providers=serde_json::from_value(json!({"isEnabled":true,"providerID":"my","adminBaseURL":"https://sub2.test/api/v1","username":"admin@example.com","password":"secret","allowInsecureTLS":false})).unwrap();
    assert_eq!(config.providers[0].provider_id, "my");
    assert_eq!(
        config.providers[0].legacy_admin_base_url,
        "https://sub2.test/api/v1"
    );
    assert!(!config.confirms("my"));
}
#[test]
fn testCompleteSub2APICredentialsEnableConnectionRegardlessOfLegacyToggle() {
    let p = Provider {
        provider_id: "my".into(),
        username: "admin@example.com".into(),
        password: "secret".into(),
        ..Default::default()
    }
    .normalized();
    assert!(p.is_enabled());
    assert!(p.is_complete());
}
