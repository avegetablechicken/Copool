import Foundation

actor SettingsCoordinator {
    private let settingsRepository: SettingsRepository
    private let launchAtStartupService: LaunchAtStartupServiceProtocol

    init(
        settingsRepository: SettingsRepository,
        launchAtStartupService: LaunchAtStartupServiceProtocol
    ) {
        self.settingsRepository = settingsRepository
        self.launchAtStartupService = launchAtStartupService
    }

    func currentSettings() throws -> AppSettings {
        try settingsRepository.loadSettings()
    }

    func updateSettings(_ patch: AppSettingsPatch) throws -> AppSettings {
        let launchAtStartupPatch = patch.launchAtStartup

        var settings = try settingsRepository.loadSettings()

        if let value = patch.launchAtStartup { settings.launchAtStartup = value }
        if let value = patch.launchCodexAfterSwitch { settings.launchCodexAfterSwitch = value }
        if let value = patch.autoSmartSwitch { settings.autoSmartSwitch = value }
        if let value = patch.syncOpencodeOpenaiAuth { settings.syncOpencodeOpenaiAuth = value }
        if let value = patch.localProxyHostAPIOnly { settings.localProxyHostAPIOnly = value }
        if let value = patch.restartEditorsOnSwitch { settings.restartEditorsOnSwitch = value }
        if let value = patch.restartEditorTargets { settings.restartEditorTargets = value }
        if let value = patch.autoStartApiProxy { settings.autoStartApiProxy = value }
        if let value = patch.proxyConfiguration { settings.proxyConfiguration = value.normalized() }
        if let value = patch.remoteServers { settings.remoteServers = value }
        if let value = patch.usageProgressDisplayMode { settings.usageProgressDisplayMode = value }
        if let value = patch.sub2APIProvider { settings.sub2APIProvider = value.normalized() }
        if let value = patch.locale { settings.locale = AppLocale.resolve(value).identifier }

        try settingsRepository.saveSettings(settings)

        if let launchAtStartupPatch {
            try launchAtStartupService.setEnabled(launchAtStartupPatch)
        }

        return settings
    }

    /// Cache refreshes may finish after an account proxy was edited.
    func updateSub2APIProviderPreservingAccountProxies(_ value: Sub2APISettingsConfiguration) throws -> AppSettings {
        let current = try settingsRepository.loadSettings().sub2APIProvider
        var value = value
        for index in value.providers.indices {
            let providerID = value.providers[index].providerID
            if let saved = current.provider(for: providerID) {
                value.providers[index].accountProxyURLs = saved.accountProxyURLs
            }
            let overrides = value.providers[index].accountProxyURLs
            value.providers[index].cachedAccounts = value.providers[index].cachedAccounts.map { account in
                var account = account
                account.proxyURL = overrides[String(account.id)]
                return account
            }
        }
        return try updateSettings(AppSettingsPatch(sub2APIProvider: value))
    }

    func syncLaunchAtStartupFromStore() throws {
        let settings = try settingsRepository.loadSettings()
        try launchAtStartupService.syncWithStoreValue(settings.launchAtStartup)
    }
}
