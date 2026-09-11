import Foundation

@MainActor
extension SettingsPageModel {
    func setLaunchAtStartup(_ value: Bool) {
        updateToggle(.launchAtStartup, to: value)
    }

    func setLaunchAfterSwitch(_ value: Bool) {
        updateToggle(.launchAfterSwitch, to: value)
    }

    func setAutoSmartSwitch(_ value: Bool) {
        updateToggle(.autoSmartSwitch, to: value)
    }

    func setAutoStartProxy(_ value: Bool) {
        updateToggle(.autoStartProxy, to: value)
    }

    func setLocale(_ value: String) {
        updateLocale(AppLocale.resolve(value))
    }

    func updateUsageProgressDisplayMode(_ value: UsageProgressDisplayMode) {
        Task { await update(AppSettingsPatch(usageProgressDisplayMode: value)) }
    }

    func setSyncOpencodeOpenaiAuth(_ value: Bool) {
        updateToggle(.syncOpencodeOpenaiAuth, to: value)
    }

    func setLocalProxyHostAPIOnly(_ value: Bool) {
        updateToggle(.localProxyHostAPIOnly, to: value)
    }

    func setRestartEditorsOnSwitch(_ value: Bool) {
        updateToggle(.restartEditorsOnSwitch, to: value)
    }

    func setRestartEditorTarget(_ target: EditorAppID?) {
        updateRestartEditorTarget(target)
    }

    func quitApp() {
        onQuitRequested()
    }

    func saveSub2APIProvider() {
        let configuration = sub2APIProviderDraft.normalized()
        guard configuration.providers.allSatisfy({
            !$0.providerID.isEmpty && $0.isComplete
        }) else {
            notice = NoticeMessage(
                style: .error,
                text: L10n.tr("error.sub2api.configuration_incomplete")
            )
            return
        }
        let normalizedProviderIDs = configuration.providers.map { $0.providerID.lowercased() }
        guard Set(normalizedProviderIDs).count == normalizedProviderIDs.count else {
            notice = NoticeMessage(
                style: .error,
                text: L10n.tr("error.sub2api.duplicate_provider")
            )
            return
        }

        isSavingSub2APIProvider = true
        Task {
            defer { isSavingSub2APIProvider = false }
            do {
                var configuration = configuration
                let current = try await settingsCoordinator.currentSettings().sub2APIProvider
                let removedConfigurationIDs = Set(current.providers.map(\.id))
                    .subtracting(configuration.providers.map(\.id))
                configuration.providers = configuration.providers.map { draft in
                    var draft = draft
                    if let saved = current.provider(for: draft.providerID) {
                        draft.importedAccountIDs = saved.importedAccountIDs
                        draft.cachedAccounts = saved.cachedAccounts
                        draft.accountProxyURLs = saved.accountProxyURLs
                    }
                    draft.legacyAdminBaseURL = ""
                    return draft
                }
                let editableConfiguration = configuration
                if let sub2APISecretStore {
                    for provider in configuration.providers {
                        try sub2APISecretStore.setPassword(provider.password, for: provider.id)
                    }
                    configuration.providers = configuration.providers.map { provider in
                        var provider = provider
                        provider.password = ""
                        return provider
                    }
                }
                settings = try await settingsCoordinator.updateSettings(
                    AppSettingsPatch(sub2APIProvider: configuration)
                )
                if let sub2APISecretStore {
                    for configurationID in removedConfigurationIDs {
                        try sub2APISecretStore.removePassword(for: configurationID)
                    }
                }
                sub2APIProviderDraft = editableConfiguration
                onSettingsUpdated(settings)
                notice = NoticeMessage(
                    style: .success,
                    text: L10n.tr("settings.notice.sub2api_saved")
                )
            } catch {
                notice = NoticeMessage(style: .error, text: error.localizedDescription)
            }
        }
    }

    func addSub2APIProviderConfiguration() {
        sub2APIProviderDraft.providers.append(
            Sub2APIProviderConfiguration()
        )
    }

    func removeSub2APIProviderConfiguration(id: UUID) {
        sub2APIProviderDraft.providers.removeAll { $0.id == id }
    }

    func updateToggle(_ intent: SettingsToggleIntent, to value: Bool) {
        switch intent {
        case .launchAtStartup:
            Task { await update(AppSettingsPatch(launchAtStartup: value)) }
        case .launchAfterSwitch:
            Task { await update(AppSettingsPatch(launchCodexAfterSwitch: value)) }
        case .autoStartProxy:
            Task { await update(AppSettingsPatch(autoStartApiProxy: value)) }
        case .localProxyHostAPIOnly:
            Task { await update(AppSettingsPatch(localProxyHostAPIOnly: value)) }
        case .autoSmartSwitch:
            Task { await update(AppSettingsPatch(autoSmartSwitch: value)) }
        case .syncOpencodeOpenaiAuth:
            Task { await update(AppSettingsPatch(syncOpencodeOpenaiAuth: value)) }
        case .restartEditorsOnSwitch:
            applyRestartEditorsOnSwitch(value)
        }
    }

    func updateRestartEditorTarget(_ target: EditorAppID?) {
        let values = target.map { [$0] } ?? []
        Task {
            await update(
                AppSettingsPatch(restartEditorTargets: values),
                successText: L10n.tr("settings.notice.restart_target_updated")
            )
        }
    }

    func updateLocale(_ locale: AppLocale) {
        Task { await update(AppSettingsPatch(locale: locale.identifier)) }
    }

    private func applyRestartEditorsOnSwitch(_ value: Bool) {
        if value,
           settings.restartEditorTargets.isEmpty,
           let first = installedEditorApps.first?.id {
            Task {
                await update(
                    AppSettingsPatch(
                        restartEditorsOnSwitch: true,
                        restartEditorTargets: [first]
                    )
                )
            }
            return
        }

        Task { await update(AppSettingsPatch(restartEditorsOnSwitch: value)) }
    }

    private func update(
        _ patch: AppSettingsPatch,
        successText: String = L10n.tr("settings.notice.updated")
    ) async {
        do {
            settings = try await settingsCoordinator.updateSettings(patch)
            onSettingsUpdated(settings)
            notice = NoticeMessage(style: .success, text: successText)
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }
}
