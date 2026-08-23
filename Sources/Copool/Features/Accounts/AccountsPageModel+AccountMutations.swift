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
        guard sub2APIAccountService.isCurrentDefaultProviderConfirmed() else {
            pendingSub2APIProviderConfirmation = sub2APIAccountService.currentDefaultProviderID()
            return
        }
        await performSub2APIImport(
            service: sub2APIAccountService,
            settingsCoordinator: settingsCoordinator
        )
    }

    func confirmSub2APIProviderAndImport(providerID: String) async {
        guard let sub2APIAccountService,
              let settingsCoordinator else { return }
        pendingSub2APIProviderConfirmation = nil

        do {
            var settings = try await settingsCoordinator.currentSettings()
            var configuration = settings.sub2APIProvider
            configuration.providerID = ""
            configuration.confirmedProviderIDs.append(providerID)
            settings = try await settingsCoordinator.updateSettings(
                AppSettingsPatch(sub2APIProvider: configuration)
            )
            onSettingsUpdated?(settings)
            await performSub2APIImport(
                service: sub2APIAccountService,
                settingsCoordinator: settingsCoordinator
            )
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func cancelPendingSub2APIProviderConfirmation() {
        pendingSub2APIProviderConfirmation = nil
    }

    private func performSub2APIImport(
        service: Sub2APIAccountServiceProtocol,
        settingsCoordinator: SettingsCoordinator
    ) async {
        guard !isImporting else { return }
        isImporting = true
        defer { isImporting = false }

        do {
            let accounts = try await service.fetchAccounts(accountIDs: nil)
            var settings = try await settingsCoordinator.currentSettings()
            var configuration = settings.sub2APIProvider
            configuration.importedAccountIDs = accounts.map(\.id)
            configuration.cachedAccounts = accounts
            settings = try await settingsCoordinator.updateSettings(
                AppSettingsPatch(sub2APIProvider: configuration)
            )
            sub2APIAccounts = accounts
            onSettingsUpdated?(settings)
            notice = NoticeMessage(
                style: .success,
                text: L10n.tr("accounts.notice.sub2api_imported_format", String(accounts.count))
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
        let configuration = settings.sub2APIProvider.normalized()
        let ids = configuration.importedAccountIDs
        guard !ids.isEmpty else {
            sub2APIAccounts = []
            return
        }
        let importedIDs = Set(ids)
        sub2APIAccounts = configuration.cachedAccounts.filter { importedIDs.contains($0.id) }

        guard let sub2APIAccountService,
              sub2APIAccountService.isCurrentDefaultProviderConfirmed(),
              let accounts = try? await sub2APIAccountService.fetchAccounts(accountIDs: ids) else {
            return
        }
        sub2APIAccounts = accounts
        var updatedConfiguration = configuration
        updatedConfiguration.cachedAccounts = accounts
        if let updatedSettings = try? await settingsCoordinator.updateSettings(
            AppSettingsPatch(sub2APIProvider: updatedConfiguration)
        ) {
            settings = updatedSettings
            onSettingsUpdated?(settings)
        }
    }

    func refreshSub2APIAccount(id: Int64) async {
        guard let sub2APIAccountService,
              !refreshingSub2APIAccountIDs.contains(id) else { return }
        refreshingSub2APIAccountIDs.insert(id)
        defer { refreshingSub2APIAccountIDs.remove(id) }

        do {
            guard let refreshed = try await sub2APIAccountService.fetchAccounts(accountIDs: [id]).first else {
                throw AppError.invalidData(L10n.tr("error.sub2api.account_not_found"))
            }
            sub2APIAccounts = sub2APIAccounts.map { $0.id == id ? refreshed : $0 }
            try await persistSub2APIAccountCache()
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func removeSub2APIAccount(id: Int64) async {
        guard let settingsCoordinator else { return }
        do {
            var settings = try await settingsCoordinator.currentSettings()
            var configuration = settings.sub2APIProvider
            configuration.importedAccountIDs.removeAll { $0 == id }
            configuration.cachedAccounts.removeAll { $0.id == id }
            settings = try await settingsCoordinator.updateSettings(
                AppSettingsPatch(sub2APIProvider: configuration)
            )
            sub2APIAccounts.removeAll { $0.id == id }
            collapsedAccountIDs.remove("sub2api-\(id)")
            onSettingsUpdated?(settings)
            notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.sub2api_removed"))
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func refreshImportedSub2APIAccounts() async throws {
        guard let sub2APIAccountService, let settingsCoordinator else { return }
        let settings = try await settingsCoordinator.currentSettings()
        let ids = settings.sub2APIProvider.importedAccountIDs
        guard !ids.isEmpty else { return }
        sub2APIAccounts = try await sub2APIAccountService.fetchAccounts(accountIDs: ids)
        try await persistSub2APIAccountCache()
    }

    private func persistSub2APIAccountCache() async throws {
        guard let settingsCoordinator else { return }
        var settings = try await settingsCoordinator.currentSettings()
        var configuration = settings.sub2APIProvider
        configuration.cachedAccounts = sub2APIAccounts
        settings = try await settingsCoordinator.updateSettings(
            AppSettingsPatch(sub2APIProvider: configuration)
        )
        onSettingsUpdated?(settings)
    }
}
