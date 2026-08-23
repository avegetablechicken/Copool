import Foundation

@MainActor
extension SettingsPageModel {
    func acceptExternalSettings(_ settings: AppSettings) {
        self.settings = settings
        guard hasLoaded else {
            sub2APIProviderDraft = settings.sub2APIProvider
            return
        }

        let incoming = settings.sub2APIProvider.normalized()
        var draft = sub2APIProviderDraft
        for index in draft.providers.indices {
            guard let saved = incoming.provider(for: draft.providers[index].providerID) else { continue }
            draft.providers[index].importedAccountIDs = saved.importedAccountIDs
            draft.providers[index].cachedAccounts = saved.cachedAccounts
        }
        sub2APIProviderDraft = draft
    }

    func acceptExternalCodexModelProviderID(_ providerID: String) {
        let providerID = providerID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !providerID.isEmpty, defaultCodexProviderID != providerID else { return }
        defaultCodexProviderID = providerID
    }

    func loadIfNeeded() async {
        if !hasLoaded {
            await load()
        }
    }

    func load() async {
        do {
            settings = try await settingsCoordinator.currentSettings()
            installedEditorApps = editorAppService.listInstalledApps()
            sub2APIProviderDraft = settings.sub2APIProvider
            if let codexConfigPath {
                let provider = CodexModelProviderResolver.resolve(configPath: codexConfigPath)
                defaultCodexProviderID = provider.id
            }
            onSettingsUpdated(settings)
            hasLoaded = true
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }
}
