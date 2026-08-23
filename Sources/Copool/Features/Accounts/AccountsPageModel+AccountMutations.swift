import Foundation
import OSLog

extension AccountsPageModel {
    private var authFlowLogger: Logger {
        Logger(subsystem: "Copool", category: "AccountsPageAuthFlow")
    }

    func importCurrentAuth() async {
        guard runtimePlatform == .macOS else {
            notice = NoticeMessage(style: .error, text: PlatformCapabilities.unsupportedOperationMessage)
            return
        }
        isImporting = true
        defer { isImporting = false }

        do {
            let imported = try await coordinator.importCurrentAuthAccount(customLabel: nil)
            let accounts = try await coordinator.listAccounts()
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: imported.id)
            publishAndSyncLocalAccountsMutation(accounts)
            notice = NoticeMessage(style: .success, text: L10n.tr("accounts.notice.imported_format", imported.label))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func addAccountViaLogin() async {
        guard addAccountTask == nil else { return }
        isAdding = true
        let task = Task { [coordinator] in
            try await coordinator.addAccountViaLogin(customLabel: nil)
        }
        addAccountTask = task
        defer {
            addAccountTask = nil
            isAdding = false
        }

        do {
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin started")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin started")
            let imported = try await task.value
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin coordinator returned \(imported.accountID, privacy: .public)")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin coordinator returned \(imported.accountID)")
            let accounts = try await coordinator.listAccounts()
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin listed \(accounts.count) accounts")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin listed \(accounts.count) accounts")
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: imported.id)
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin refreshed pending workspaces")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin refreshed pending workspaces")
            publishAndSyncLocalAccountsMutation(accounts)
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin published local mutation")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin published local mutation")
            notice = NoticeMessage(style: .success, text: L10n.tr("accounts.notice.imported_new_format", imported.label))
        } catch is CancellationError {
            authFlowLogger.log("AccountsPageModel.addAccountViaLogin cancelled")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin cancelled")
            notice = NoticeMessage(style: .error, text: L10n.tr("error.oauth.request_cancelled"))
        } catch {
            authFlowLogger.error("AccountsPageModel.addAccountViaLogin failed: \(error.localizedDescription, privacy: .public)")
            AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.addAccountViaLogin failed: \(error.localizedDescription)")
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func reauthenticateAccount(id: String) async {
        guard runtimePlatform == .macOS else {
            notice = NoticeMessage(style: .error, text: PlatformCapabilities.unsupportedOperationMessage)
            return
        }
        guard addAccountTask == nil else { return }
        guard case .content(let accounts) = state,
              let account = accounts.first(where: { $0.id == id }) else {
            return
        }

        isAdding = true
        let task = Task { [coordinator] in
            if account.shouldDisplayWorkspaceTag, let workspaceName = account.displayTeamName {
                return try await coordinator.authorizeWorkspaceViaLogin(
                    workspaceID: account.accountID,
                    workspaceName: workspaceName,
                    customLabel: nil
                )
            }
            return try await coordinator.addAccountViaLogin(customLabel: nil)
        }
        addAccountTask = task
        defer {
            addAccountTask = nil
            isAdding = false
        }

        do {
            let imported = try await task.value
            let accounts = try await coordinator.listAccounts()
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: imported.id)
            publishAndSyncLocalAccountsMutation(accounts)
            notice = NoticeMessage(style: .success, text: L10n.tr("accounts.notice.imported_format", imported.label))
        } catch is CancellationError {
            notice = NoticeMessage(style: .error, text: L10n.tr("error.oauth.request_cancelled"))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func cancelAddAccount() {
        authFlowLogger.log("AccountsPageModel.cancelAddAccount requested")
        AuthFlowDebugLog.write("AccountsPageAuthFlow", "AccountsPageModel.cancelAddAccount requested")
        addAccountTask?.cancel()
    }

    func importAuthDocument(from url: URL, setAsCurrent: Bool) async {
        if setAsCurrent {
            isImporting = true
        } else {
            isAdding = true
        }
        defer {
            if setAsCurrent {
                isImporting = false
            } else {
                isAdding = false
            }
        }

        do {
            let imported = try await coordinator.importAccountFile(
                from: url,
                customLabel: nil,
                setAsCurrent: setAsCurrent
            )
            let accounts = try await coordinator.listAccounts()
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: imported.id)
            publishAndSyncLocalAccountsMutation(accounts)
            let key = setAsCurrent
                ? "accounts.notice.imported_format"
                : "accounts.notice.imported_new_format"
            notice = NoticeMessage(style: .success, text: L10n.tr(key, imported.label))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func deleteAccount(id: String) async {
        do {
            try await coordinator.deleteAccount(id: id)
            let accounts = try await coordinator.listAccounts()
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts)
            publishAndSyncLocalAccountsMutation(accounts)
            notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.account_deleted"))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func saveTeamAlias(id: String, alias: String?) async {
        do {
            _ = try await coordinator.updateTeamAlias(id: id, alias: alias)
            let accounts = try await coordinator.listAccounts()
            applyAccounts(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts)
            publishAndSyncLocalAccountsMutation(accounts)
            notice = NoticeMessage(style: .success, text: L10n.tr("accounts.notice.team_name_updated"))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }
}

@MainActor
extension AccountsPageModel {
    func importSub2APIAccounts() async {
        guard let sub2APIAccountService, let settingsCoordinator else {
            notice = NoticeMessage(style: .error, text: L10n.tr("error.sub2api.configuration_incomplete"))
            return
        }
        guard let settings = try? await settingsCoordinator.currentSettings() else {
            notice = NoticeMessage(style: .error, text: L10n.tr("error.sub2api.configuration_incomplete"))
            return
        }
        let sub2APISettings = settings.sub2APIProvider.normalized()
        guard !sub2APISettings.providers.isEmpty,
              sub2APISettings.providers.allSatisfy({ $0.isEnabled }) else {
            notice = NoticeMessage(style: .error, text: L10n.tr("error.sub2api.configuration_incomplete"))
            return
        }
        await performSub2APIImport(
            service: sub2APIAccountService,
            settingsCoordinator: settingsCoordinator
        )
    }

    private func performSub2APIImport(
        service: Sub2APIAccountServiceProtocol,
        settingsCoordinator: SettingsCoordinator
    ) async {
        guard !isImporting else { return }
        isImporting = true
        defer { isImporting = false }

        do {
            var settings = try await settingsCoordinator.currentSettings()
            var sub2APISettings = settings.sub2APIProvider.normalized()
            guard !sub2APISettings.providers.isEmpty else {
                throw AppError.invalidData(L10n.tr("error.sub2api.configuration_incomplete"))
            }
            var synchronizedAccounts = sub2APIAccounts
            var synchronizedCount = 0
            for rawConfiguration in sub2APISettings.providers {
                var configuration = rawConfiguration
                let providerID = configuration.providerID
                let accounts = try await service.fetchAccounts(
                    providerID: providerID,
                    accountIDs: nil
                ).map {
                    $0.settingProvider(providerID)
                }
                configuration.importedAccountIDs = accounts.map(\.id)
                configuration.cachedAccounts = accounts
                sub2APISettings.upsert(configuration)
                synchronizedAccounts.removeAll {
                    $0.providerID?.caseInsensitiveCompare(providerID) == .orderedSame
                }
                synchronizedAccounts.append(contentsOf: accounts)
                synchronizedCount += accounts.count
            }
            settings = try await settingsCoordinator.updateSettings(
                AppSettingsPatch(sub2APIProvider: sub2APISettings)
            )
            sub2APIAccounts = synchronizedAccounts
            publishSub2APIAccounts()
            onSettingsUpdated?(settings)
            notice = NoticeMessage(
                style: .success,
                text: L10n.tr("accounts.notice.sub2api_imported_format", String(synchronizedCount))
            )
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func loadImportedSub2APIAccounts() async {
        guard let settingsCoordinator,
              var settings = try? await settingsCoordinator.currentSettings() else {
            sub2APIAccounts = []
            return
        }
        var sub2APISettings = settings.sub2APIProvider.normalized()
        let cachedAccounts = sub2APISettings.providers.flatMap { configuration in
            configuration.cachedAccounts.map { $0.settingProvider(configuration.providerID) }
        }
        sub2APIAccounts = cachedAccounts
        publishSub2APIAccounts()

        guard let sub2APIAccountService,
              sub2APIAccountService.canQueryCurrentDefaultProvider(),
              let configuration = sub2APISettings.provider(
                  for: sub2APIAccountService.currentDefaultProviderID()
              ),
              !configuration.importedAccountIDs.isEmpty,
              let accounts = try? await sub2APIAccountService.fetchAccounts(
                  accountIDs: configuration.importedAccountIDs
              ) else {
            return
        }
        let providerID = configuration.providerID
        let associatedAccounts = accounts.map {
            $0.settingProvider(providerID)
        }
        sub2APIAccounts.removeAll {
            $0.providerID?.caseInsensitiveCompare(providerID) == .orderedSame
        }
        sub2APIAccounts.append(contentsOf: associatedAccounts)
        publishSub2APIAccounts()
        var updatedConfiguration = configuration
        updatedConfiguration.cachedAccounts = associatedAccounts
        sub2APISettings.upsert(updatedConfiguration)
        if let updatedSettings = try? await settingsCoordinator.updateSettings(
            AppSettingsPatch(sub2APIProvider: sub2APISettings)
        ) {
            settings = updatedSettings
            onSettingsUpdated?(settings)
        }
    }

    func refreshSub2APIAccount(_ account: Sub2APIAccountSummary) async {
        guard let sub2APIAccountService,
              !refreshingSub2APIAccountIDs.contains(account.cardID) else { return }
        refreshingSub2APIAccountIDs.insert(account.cardID)
        defer { refreshingSub2APIAccountIDs.remove(account.cardID) }

        do {
            guard account.providerID?.caseInsensitiveCompare(
                sub2APIAccountService.currentDefaultProviderID()
            ) == .orderedSame else {
                throw AppError.invalidData(L10n.tr("error.sub2api.provider_not_confirmed"))
            }
            guard let refreshed = try await sub2APIAccountService.fetchAccounts(
                accountIDs: [account.id]
            ).first else {
                throw AppError.invalidData(L10n.tr("error.sub2api.account_not_found"))
            }
            let associated = refreshed.settingProvider(sub2APIAccountService.currentDefaultProviderID())
            sub2APIAccounts = sub2APIAccounts.map { $0.cardID == account.cardID ? associated : $0 }
            publishSub2APIAccounts()
            try await persistSub2APIAccountCache()
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func removeSub2APIAccount(_ account: Sub2APIAccountSummary) async {
        guard let settingsCoordinator else { return }
        do {
            var settings = try await settingsCoordinator.currentSettings()
            var sub2APISettings = settings.sub2APIProvider.normalized()
            guard var configuration = account.providerID.flatMap({
                sub2APISettings.provider(for: $0)
            }) ?? sub2APISettings.provider(containingAccountID: account.id) else {
                return
            }
            configuration.importedAccountIDs.removeAll { $0 == account.id }
            configuration.cachedAccounts.removeAll { $0.id == account.id }
            sub2APISettings.upsert(configuration)
            settings = try await settingsCoordinator.updateSettings(
                AppSettingsPatch(sub2APIProvider: sub2APISettings)
            )
            sub2APIAccounts.removeAll { $0.cardID == account.cardID }
            publishSub2APIAccounts()
            collapsedAccountIDs.remove(account.cardID)
            onSettingsUpdated?(settings)
            notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.sub2api_removed"))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func refreshImportedSub2APIAccounts() async throws {
        guard let sub2APIAccountService, let settingsCoordinator else { return }
        let settings = try await settingsCoordinator.currentSettings()
        let providerID = sub2APIAccountService.currentDefaultProviderID()
        guard let configuration = settings.sub2APIProvider.provider(for: providerID),
              !configuration.importedAccountIDs.isEmpty else { return }
        let refreshed = try await sub2APIAccountService.fetchAccounts(
            accountIDs: configuration.importedAccountIDs
        ).map {
            $0.settingProvider(providerID)
        }
        sub2APIAccounts.removeAll {
            $0.providerID?.caseInsensitiveCompare(providerID) == .orderedSame
        }
        sub2APIAccounts.append(contentsOf: refreshed)
        publishSub2APIAccounts()
        try await persistSub2APIAccountCache()
    }

    func persistSub2APIAccountCache() async throws {
        guard let settingsCoordinator else { return }
        var settings = try await settingsCoordinator.currentSettings()
        var sub2APISettings = settings.sub2APIProvider.normalized()
        for index in sub2APISettings.providers.indices {
            let providerID = sub2APISettings.providers[index].providerID
            let importedIDs = Set(sub2APISettings.providers[index].importedAccountIDs)
            sub2APISettings.providers[index].cachedAccounts = sub2APIAccounts.filter {
                $0.providerID?.caseInsensitiveCompare(providerID) == .orderedSame
                    && importedIDs.contains($0.id)
            }
        }
        settings = try await settingsCoordinator.updateSettings(
            AppSettingsPatch(sub2APIProvider: sub2APISettings)
        )
        onSettingsUpdated?(settings)
    }

    func publishSub2APIAccounts() {
        onSub2APIAccountsChanged?(sub2APIAccounts)
    }
}
