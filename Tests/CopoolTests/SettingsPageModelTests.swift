import XCTest
@testable import Copool

@MainActor
final class SettingsPageModelTests: XCTestCase {
    func testCacheRefreshCannotOverwriteNewAccountProxy() async throws {
        let provider = Sub2APIProviderConfiguration(
            providerID: "p", importedAccountIDs: [42]
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(providers: [provider])
        let stale = settings.sub2APIProvider
        settings.sub2APIProvider.providers[0].accountProxyURLs["42"] = "socks5://127.0.0.1:1080"
        let repository = TestSettingsRepository(settings: settings)
        let coordinator = SettingsCoordinator(
            settingsRepository: repository, launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let updated = try await coordinator.updateSub2APIProviderPreservingAccountProxies(stale)
        XCTAssertEqual(updated.sub2APIProvider.providers[0].proxyURL(forAccountID: 42), "socks5://127.0.0.1:1080")
    }

    func testQuitAppInvokesInjectedAction() {
        var didQuit = false
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: TestSettingsRepository(),
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService(),
            onQuitRequested: {
                didQuit = true
            }
        )

        model.quitApp()

        XCTAssertTrue(didQuit)
    }

    func testSaveSub2APIProviderStoresPasswordOutsideSettings() async throws {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let secretStore = SettingsStubSub2APISecretStore()
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService(),
            sub2APISecretStore: secretStore
        )
        model.sub2APIProviderDraft = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "admin@example.com",
                    password: "secret",
                    allowInsecureTLS: true
                )
            ]
        )

        model.saveSub2APIProvider()
        while model.isSavingSub2APIProvider {
            await Task.yield()
        }

        let configurationID = try XCTUnwrap(model.sub2APIProviderDraft.providers.first?.id)
        let stored = try XCTUnwrap(
            settingsRepository.loadSettings().sub2APIProvider.providers.first
        )
        XCTAssertEqual(stored.username, "admin@example.com")
        XCTAssertEqual(stored.password, "")
        XCTAssertEqual(try secretStore.password(for: configurationID), "secret")
        XCTAssertEqual(model.sub2APIProviderDraft.providers.first?.password, "secret")
        XCTAssertEqual(model.notice?.text, L10n.tr("settings.notice.sub2api_saved"))

        let reloadedModel = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService(),
            sub2APISecretStore: secretStore
        )
        await reloadedModel.load()
        XCTAssertEqual(reloadedModel.sub2APIProviderDraft.providers.first?.password, "secret")
    }

    func testSettingsPageAddsAndSavesArbitrarySub2APIProviderConfigurations() async throws {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService()
        )

        await model.load()
        model.addSub2APIProviderConfiguration()
        model.addSub2APIProviderConfiguration()
        model.addSub2APIProviderConfiguration()
        XCTAssertEqual(model.sub2APIProviderDraft.providers.count, 3)
        let providerIDs = ["my", "ShareCoder", "custom-provider"]
        for index in model.sub2APIProviderDraft.providers.indices {
            model.sub2APIProviderDraft.providers[index].providerID = providerIDs[index]
            model.sub2APIProviderDraft.providers[index].username = "admin-\(index)@example.com"
            model.sub2APIProviderDraft.providers[index].password = "secret-\(index)"
        }

        model.saveSub2APIProvider()
        while model.isSavingSub2APIProvider {
            await Task.yield()
        }

        let saved = try settingsRepository.loadSettings().sub2APIProvider
        XCTAssertEqual(Set(saved.providers.map(\.providerID)), Set(providerIDs))
        XCTAssertTrue(saved.providers.allSatisfy { $0.legacyAdminBaseURL.isEmpty })
    }

    func testLegacySub2APIConfigurationUsesCodexBaseURLDuringMigration() throws {
        let configPath = FileManager.default.temporaryDirectory
            .appendingPathComponent("settings-sub2api-migration-\(UUID().uuidString).toml")
        try """
        model_provider = "my"

        [model_providers.my]
        base_url = "https://my-sub2.test:6060"

        [model_providers.ShareCoder]
        base_url = "https://sharecoder.test"
        """.write(to: configPath, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: configPath) }
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "codex@example.com",
            accountID: "account-42",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil
        )
        let legacy = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["ShareCoder", "my"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "admin@example.com",
                    password: "secret",
                    importedAccountIDs: [42],
                    cachedAccounts: [account],
                    legacyAdminBaseURL: "https://sharecoder.test/api/v1"
                )
            ]
        )

        let secretStore = SettingsStubSub2APISecretStore()
        let matched = AppContainer.migrateSub2APISettings(legacy, configPath: configPath)
        let migrated = try AppContainer.migrateSub2APIPasswords(
            matched,
            secretStore: secretStore
        )

        XCTAssertEqual(migrated.providers.first?.providerID, "ShareCoder")
        let configurationID = try XCTUnwrap(migrated.providers.first?.id)
        XCTAssertEqual(migrated.providers.first?.password, "")
        XCTAssertEqual(try secretStore.password(for: configurationID), "secret")
        XCTAssertEqual(migrated.providers.first?.legacyAdminBaseURL, "")
        XCTAssertEqual(migrated.providers.first?.cachedAccounts.first?.providerID, "ShareCoder")
        XCTAssertEqual(Set(migrated.confirmedProviderIDs), ["ShareCoder", "my"])
    }

    func testAccountsPageImportsSub2APIAccountsAndPersistsIDs() async throws {
        var initialSettings = AppSettings.defaultValue
        initialSettings.sub2APIProvider = makeSub2APISettings(providerID: "my")
        let settingsRepository = TestSettingsRepository(settings: initialSettings)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "codex@example.com",
            email: "codex@example.com",
            accountID: "chatgpt-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: UsageSnapshot(
                fetchedAt: 1,
                planType: "pro",
                fiveHour: UsageWindow(usedPercent: 20, windowSeconds: 18_000, resetAt: 100),
                oneWeek: UsageWindow(usedPercent: 30, windowSeconds: 604_800, resetAt: 200),
                credits: nil
            ),
            usageError: nil
        )
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(accounts: [account]),
            initialAccounts: []
        )

        await model.importSub2APIAccounts()

        let associatedAccount = account.associatingProviderIfMissing("my")
        XCTAssertEqual(model.sub2APIAccounts, [associatedAccount])
        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider.provider(for: "my")?.importedAccountIDs,
            [42]
        )
        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider.provider(for: "my")?.cachedAccounts,
            [associatedAccount]
        )
        guard case .content = model.makeContentPresentation().state else {
            return XCTFail("Sub2api accounts should make the page display content")
        }
    }

    func testAccountsPageSub2APIImportPreservesOtherProviderCaches() async throws {
        let shareAccount = Sub2APIAccountSummary(
            id: 7,
            name: "sharecoder-account",
            email: "share@example.com",
            accountID: "share-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "ShareCoder"
        )
        let myAccount = Sub2APIAccountSummary(
            id: 42,
            name: "my-account",
            email: "my@example.com",
            accountID: "my-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil
        )
        var refreshedShareAccount = shareAccount
        refreshedShareAccount.email = "share-refreshed@example.com"
        var initialSettings = AppSettings.defaultValue
        initialSettings.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my", "ShareCoder"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "my-admin@example.com",
                    password: "my-secret"
                ),
                Sub2APIProviderConfiguration(
                    providerID: "ShareCoder",
                    username: "share-admin@example.com",
                    password: "share-secret",
                    importedAccountIDs: [7],
                    cachedAccounts: [shareAccount]
                ),
            ]
        )
        let settingsRepository = TestSettingsRepository(settings: initialSettings)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [myAccount],
                providerID: "my",
                accountsByProviderID: [
                    "my": [myAccount],
                    "ShareCoder": [refreshedShareAccount],
                ]
            ),
            initialAccounts: [],
            initialSub2APIAccounts: [shareAccount]
        )

        await model.importSub2APIAccounts()

        let settings = try settingsRepository.loadSettings().sub2APIProvider
        XCTAssertEqual(
            settings.provider(for: "ShareCoder")?.cachedAccounts,
            [refreshedShareAccount]
        )
        XCTAssertEqual(
            settings.provider(for: "my")?.cachedAccounts,
            [myAccount.settingProvider("my")]
        )
        XCTAssertEqual(Set(model.sub2APIAccounts.map(\.cardID)), [
            shareAccount.cardID,
            myAccount.settingProvider("my").cardID,
        ])
    }

    func testAccountsPageSub2APIImportRequiresCurrentProviderConfiguration() async {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [],
                providerID: "my",
                isConnected: false
            ),
            initialAccounts: []
        )

        await model.importSub2APIAccounts()

        XCTAssertEqual(model.notice?.text, L10n.tr("error.sub2api.configuration_incomplete"))
    }

    func testManualSub2APIRefreshUsesCardProviderWithoutConfirmation() async throws {
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "my-account",
            email: "old@example.com",
            accountID: "my-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "my"
        )
        var refreshed = account
        refreshed.email = "refreshed@example.com"
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "my-admin@example.com",
                    password: "secret",
                    importedAccountIDs: [42],
                    cachedAccounts: [account]
                ),
                Sub2APIProviderConfiguration(
                    providerID: "ShareCoder",
                    username: "share-admin@example.com",
                    password: "secret"
                ),
            ]
        )
        let settingsRepository = TestSettingsRepository(settings: settings)
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [],
                providerID: "ShareCoder",
                accountsByProviderID: ["my": [refreshed]]
            ),
            initialAccounts: [],
            initialSub2APIAccounts: [account]
        )

        await model.refreshSub2APIAccount(account)

        XCTAssertEqual(model.sub2APIAccounts.first?.displayEmail, "refreshed@example.com")
        XCTAssertNil(model.notice)
    }

    func testAccountsPageRestoresCachedSub2APIAccountsWithoutNetwork() async throws {
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "codex@example.com",
            accountID: "chatgpt-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = makeSub2APISettings(
            providerID: "ShareCoder",
            importedAccountIDs: [42],
            cachedAccounts: [account]
        )
        let settingsRepository = TestSettingsRepository(settings: settings)
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            sub2APIAccountService: nil,
            initialAccounts: [],
            initialSub2APIAccounts: [account]
        )

        XCTAssertEqual(model.sub2APIAccounts, [account])
        await model.loadImportedSub2APIAccounts()

        XCTAssertEqual(
            model.sub2APIAccounts,
            [account.associatingProviderIfMissing("ShareCoder")]
        )
    }

    func testTrayRecurringRefreshUpdatesSub2APIAccountsAndCache() async throws {
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "codex@example.com",
            accountID: "chatgpt-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = makeSub2APISettings(
            providerID: "ShareCoder",
            importedAccountIDs: [42]
        )
        let settingsRepository = TestSettingsRepository(settings: settings)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let coordinator = AccountsCoordinator(
            storeRepository: SettingsTestAccountsStoreRepository(),
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        let trayModel = TrayMenuModel(
            accountsCoordinator: coordinator,
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [account],
                providerID: "ShareCoder"
            ),
            backgroundRefreshPolicy: .init(
                initialRefreshDelay: .seconds(1),
                usageRefreshInterval: .seconds(10),
                refreshUsageOnRecurringTick: true
            )
        )

        await trayModel.refreshRecurringUsage(tick: 0)

        let associatedAccount = account.settingProvider("ShareCoder")
        XCTAssertEqual(trayModel.sub2APIAccounts, [associatedAccount])
        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider.provider(for: "ShareCoder")?.cachedAccounts,
            [associatedAccount]
        )
    }

    func testManualRefreshUpdatesAllSub2APIProvidersWithOfficialDefault() async throws {
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "codex@example.com",
            accountID: "chatgpt-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = makeSub2APISettings(
            providerID: "ShareCoder",
            importedAccountIDs: [42]
        )
        settings.sub2APIProvider.upsert(Sub2APIProviderConfiguration(
            providerID: "second", username: "admin", password: "secret",
            importedAccountIDs: [42], cachedAccounts: [account.settingProvider("second")]
        ))
        let service = SettingsStubSub2APIAccountService(
            accounts: [], providerID: "openai",
            accountsByProviderID: ["ShareCoder": [account], "second": [account]],
            isConnected: false
        )
        let settingsRepository = TestSettingsRepository(settings: settings)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let coordinator = AccountsCoordinator(
            storeRepository: SettingsTestAccountsStoreRepository(),
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        let trayModel = TrayMenuModel(
            accountsCoordinator: coordinator,
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: service,
            backgroundRefreshPolicy: .init(
                initialRefreshDelay: .seconds(1),
                usageRefreshInterval: .seconds(10),
                refreshUsageOnRecurringTick: true
            )
        )

        _ = try await trayModel.performManualRefresh(onPartialUpdate: { _ in })
        trayModel.stopBackgroundRefresh()

        let expected = [account.settingProvider("ShareCoder"), account.settingProvider("second")]
        XCTAssertEqual(Set(trayModel.sub2APIAccounts.map(\.cardID)), Set(expected.map(\.cardID)))
        for account in expected {
            XCTAssertEqual(
                try settingsRepository.loadSettings().sub2APIProvider.provider(for: account.providerID!)?.cachedAccounts,
                [account]
            )
        }

        // The accounts page also supports refreshing without the tray service.
        let pageModel = AccountsPageModel(
            coordinator: coordinator,
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: service,
            initialAccounts: []
        )
        await pageModel.refreshUsage()
        XCTAssertEqual(Set(pageModel.sub2APIAccounts.map(\.cardID)), Set(expected.map(\.cardID)))
        XCTAssertEqual(pageModel.notice?.style, .info)
    }

    func testAccountsPageSyncsConfiguredProviderWithoutConfirmation() async throws {
        var initialSettings = AppSettings.defaultValue
        initialSettings.sub2APIProvider = makeSub2APISettings(
            providerID: "ShareCoder",
            confirmed: false
        )
        let settingsRepository = TestSettingsRepository(settings: initialSettings)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let account = Sub2APIAccountSummary(
            id: 7,
            name: "sharecoder@example.com",
            email: nil,
            accountID: nil,
            accountType: "oauth",
            status: "active",
            planType: nil,
            usage: nil,
            usageError: nil
        )
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            settingsCoordinator: settingsCoordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [account],
                providerID: "ShareCoder",
                isConnected: true
            ),
            initialAccounts: []
        )

        await model.importSub2APIAccounts()

        XCTAssertEqual(
            model.sub2APIAccounts,
            [account.associatingProviderIfMissing("ShareCoder")]
        )
    }

    func testAccountsPageModelToggleUsageProgressDisplayPersistsAndShowsNotice() async {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let coordinator = AccountsCoordinator(
            storeRepository: SettingsTestAccountsStoreRepository(),
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        let model = AccountsPageModel(
            coordinator: coordinator,
            settingsCoordinator: settingsCoordinator
        )

        await model.handlePageAction(AccountsPageActionIntent.toggleUsageProgressDisplay)

        XCTAssertEqual(model.usageProgressDisplayMode, UsageProgressDisplayMode.remaining)
        XCTAssertEqual(
            try? settingsRepository.loadSettings().usageProgressDisplayMode,
            UsageProgressDisplayMode.remaining
        )
        XCTAssertEqual(
            model.notice?.text,
            L10n.tr(
                "accounts.notice.usage_progress_display_changed_format",
                L10n.tr("settings.usage_progress_display.remaining")
            )
        )
    }

    func testAccountsPageModelToggleUsageProgressDisplayInvokesSettingsUpdateCallback() async {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let settingsCoordinator = SettingsCoordinator(
            settingsRepository: settingsRepository,
            launchAtStartupService: SettingsStubLaunchAtStartupService()
        )
        let coordinator = AccountsCoordinator(
            storeRepository: SettingsTestAccountsStoreRepository(),
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        var callbackMode: UsageProgressDisplayMode?
        let model = AccountsPageModel(
            coordinator: coordinator,
            settingsCoordinator: settingsCoordinator,
            onSettingsUpdated: { settings in
                callbackMode = settings.usageProgressDisplayMode
            }
        )

        await model.handlePageAction(AccountsPageActionIntent.toggleUsageProgressDisplay)

        XCTAssertEqual(callbackMode, .remaining)
    }

    func testAccountsPageSwitchesToSub2APIAccountProvider() async throws {
        let providerSwitchService = SettingsStubCodexModelProviderSwitchService()
        let opencodeSyncService = SettingsRecordingOpencodeAuthSyncService()
        var changedProviderID: String?
        let account = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "codex@example.com",
            accountID: "chatgpt-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "ShareCoder"
        )
        var providerSettings = AppSettings.defaultValue
        providerSettings.launchCodexAfterSwitch = false
        providerSettings.syncOpencodeOpenaiAuth = true
        providerSettings.restartEditorsOnSwitch = true
        providerSettings.restartEditorTargets = [.cursor]
        let settingsRepository = TestSettingsRepository(settings: providerSettings)
        let model = AccountsPageModel(
            coordinator: AccountsCoordinator(
                storeRepository: SettingsTestAccountsStoreRepository(),
                settingsRepository: settingsRepository,
                authRepository: SettingsTestAuthRepository(),
                usageService: SettingsTestUsageService(),
                chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
                codexCLIService: SettingsStubCodexCLIService(),
                editorAppService: SettingsStubEditorAppService(),
                opencodeAuthSyncService: opencodeSyncService,
                dateProvider: SettingsFixedDateProvider(now: 1)
            ),
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [account],
                providerID: "my",
                configuredProvider: "ShareCoder"
            ),
            codexModelProviderSwitchService: providerSwitchService,
            onCodexModelProviderChanged: { providerID in
                changedProviderID = providerID
            },
            initialAccounts: [],
            initialSub2APIAccounts: [account]
        )

        await model.switchSub2APIProvider(account: account)

        XCTAssertEqual(providerSwitchService.switchedProviderIDs, ["ShareCoder"])
        XCTAssertEqual(model.currentCodexModelProviderID, "ShareCoder")
        XCTAssertEqual(changedProviderID, "ShareCoder")
        XCTAssertEqual(opencodeSyncService.callCount, 0)
        let card = try XCTUnwrap(model.makeSub2APIAccountCardViewStates().first?.card)
        XCTAssertTrue(card.account.isCurrent)
        XCTAssertEqual(card.presentation.teamNameTag, "SUB2API")
        XCTAssertEqual(
            model.notice?.text,
            [
                L10n.tr("accounts.notice.provider_switched_format", "ShareCoder"),
                L10n.tr("accounts.notice.editor_restarted_format", EditorAppID.cursor.rawValue),
            ].joined(separator: " · ")
        )
        XCTAssertNil(model.switchingAccountID)
    }

    func testAccountsPageSwitchesProviderToOpenAIForLocalAccount() async throws {
        let providerSwitchService = SettingsStubCodexModelProviderSwitchService()
        let account = StoredAccount(
            id: "local-account",
            label: "local@example.com",
            email: "local@example.com",
            accountID: "chatgpt-account",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 1,
            usage: nil,
            usageError: nil
        )
        let storeRepository = SettingsTestAccountsStoreRepository(
            store: AccountsStore(accounts: [account])
        )
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let coordinator = AccountsCoordinator(
            storeRepository: storeRepository,
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        let model = AccountsPageModel(
            coordinator: coordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [],
                providerID: "ShareCoder",
                configuredProvider: "ShareCoder"
            ),
            codexModelProviderSwitchService: providerSwitchService,
            initialAccounts: try await coordinator.listAccounts(refreshWorkspaceMetadata: false)
        )

        XCTAssertFalse(try XCTUnwrap(model.makeAccountCardViewStates().first).account.isCurrent)

        await model.switchAccount(id: account.id)

        XCTAssertEqual(providerSwitchService.switchedProviderIDs, ["openai"])
        XCTAssertEqual(model.currentCodexModelProviderID, "openai")
        XCTAssertTrue(try XCTUnwrap(model.makeAccountCardViewStates().first).account.isCurrent)
        XCTAssertEqual(try storeRepository.loadStore().currentAccountID, account.id)
    }

    func testSettingsPageAcceptsProviderChangedByAccountCard() {
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: TestSettingsRepository(),
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService()
        )

        model.acceptExternalCodexModelProviderID("ShareCoder")

        XCTAssertEqual(model.defaultCodexProviderID, "ShareCoder")
    }

    func testBackgroundCacheUpdatePreservesEditedSub2APICredentials() {
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: TestSettingsRepository(),
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService()
        )
        model.hasLoaded = true
        model.sub2APIProviderDraft = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "ShareCoder",
                    username: "editing@example.com",
                    password: "editing-secret"
                )
            ]
        )
        let cachedAccount = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "cached@example.com",
            accountID: "account-42",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "ShareCoder"
        )
        var incoming = AppSettings.defaultValue
        incoming.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["ShareCoder"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "ShareCoder",
                    username: "saved@example.com",
                    password: "saved-secret",
                    importedAccountIDs: [42],
                    cachedAccounts: [cachedAccount]
                )
            ]
        )

        model.acceptExternalSettings(incoming)

        XCTAssertEqual(model.sub2APIProviderDraft.providers.first?.username, "editing@example.com")
        XCTAssertEqual(model.sub2APIProviderDraft.providers.first?.password, "editing-secret")
        XCTAssertEqual(model.sub2APIProviderDraft.providers.first?.cachedAccounts, [cachedAccount])
    }

    func testSmartSwitchIncludesSub2APIProviderCandidate() async throws {
        let providerSwitchService = SettingsStubCodexModelProviderSwitchService()
        let localAccount = StoredAccount(
            id: "local-account",
            label: "local@example.com",
            email: "local@example.com",
            accountID: "chatgpt-account",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 1,
            usage: UsageSnapshot(
                fetchedAt: 1,
                planType: "pro",
                fiveHour: UsageWindow(usedPercent: 90, windowSeconds: 18_000, resetAt: nil),
                oneWeek: UsageWindow(usedPercent: 90, windowSeconds: 604_800, resetAt: nil),
                credits: nil
            ),
            usageError: nil
        )
        let sub2APIAccount = Sub2APIAccountSummary(
            id: 42,
            name: "openai-2026",
            email: "sub2api@example.com",
            accountID: "sub2api-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: UsageSnapshot(
                fetchedAt: 1,
                planType: "pro",
                fiveHour: UsageWindow(usedPercent: 10, windowSeconds: 18_000, resetAt: nil),
                oneWeek: UsageWindow(usedPercent: 10, windowSeconds: 604_800, resetAt: nil),
                credits: nil
            ),
            usageError: nil,
            providerID: "ShareCoder"
        )
        let storeRepository = SettingsTestAccountsStoreRepository(
            store: AccountsStore(accounts: [localAccount], currentAccountID: localAccount.id)
        )
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let coordinator = AccountsCoordinator(
            storeRepository: storeRepository,
            settingsRepository: settingsRepository,
            authRepository: SettingsTestAuthRepository(),
            usageService: SettingsTestUsageService(),
            chatGPTOAuthLoginService: SettingsStubChatGPTOAuthLoginService(),
            codexCLIService: SettingsStubCodexCLIService(),
            editorAppService: SettingsStubEditorAppService(),
            opencodeAuthSyncService: SettingsStubOpencodeAuthSyncService(),
            dateProvider: SettingsFixedDateProvider(now: 1)
        )
        let model = AccountsPageModel(
            coordinator: coordinator,
            sub2APIAccountService: SettingsStubSub2APIAccountService(
                accounts: [sub2APIAccount],
                providerID: "openai",
                configuredProvider: "ShareCoder"
            ),
            codexModelProviderSwitchService: providerSwitchService,
            initialAccounts: try await coordinator.listAccounts(refreshWorkspaceMetadata: false),
            initialSub2APIAccounts: [sub2APIAccount]
        )

        await model.smartSwitch()

        XCTAssertEqual(providerSwitchService.switchedProviderIDs, ["ShareCoder"])
        XCTAssertEqual(model.currentCodexModelProviderID, "ShareCoder")
        XCTAssertTrue(try XCTUnwrap(model.makeSub2APIAccountCardViewStates().first).card.account.isCurrent)
    }

}

private func makeSub2APISettings(
    providerID: String,
    confirmed: Bool = true,
    importedAccountIDs: [Int64] = [],
    cachedAccounts: [Sub2APIAccountSummary] = []
) -> Sub2APISettingsConfiguration {
    Sub2APISettingsConfiguration(
        confirmedProviderIDs: confirmed ? [providerID] : [],
        providers: [
            Sub2APIProviderConfiguration(
                providerID: providerID,
                username: "admin@example.com",
                password: "secret",
                importedAccountIDs: importedAccountIDs,
                cachedAccounts: cachedAccounts
            )
        ]
    )
}

final class TestSettingsRepository: SettingsRepository, @unchecked Sendable {
    private var settings: AppSettings

    init(settings: AppSettings = .defaultValue) {
        self.settings = settings
    }

    func loadSettings() throws -> AppSettings {
        settings
    }

    func saveSettings(_ settings: AppSettings) throws {
        self.settings = settings
    }
}

private struct SettingsStubLaunchAtStartupService: LaunchAtStartupServiceProtocol {
    func setEnabled(_ enabled: Bool) throws {
        _ = enabled
    }

    func syncWithStoreValue(_ enabled: Bool) throws {
        _ = enabled
    }
}

private struct SettingsStubEditorAppService: EditorAppServiceProtocol {
    func listInstalledApps() -> [InstalledEditorApp] {
        []
    }

    func restartSelectedApps(_ targets: [EditorAppID]) -> (restarted: [EditorAppID], error: String?) {
        (targets, nil)
    }
}

private final class SettingsTestAccountsStoreRepository: AccountsStoreRepository, @unchecked Sendable {
    private var store: AccountsStore

    init(store: AccountsStore = AccountsStore()) {
        self.store = store
    }

    func loadStore() throws -> AccountsStore {
        store
    }

    func saveStore(_ store: AccountsStore) throws {
        self.store = store
    }
}

private struct SettingsTestAuthRepository: AuthRepository {
    func readCurrentAuth() throws -> JSONValue { .object([:]) }
    func readCurrentAuthOptional() throws -> JSONValue? { nil }
    func readAuth(from url: URL) throws -> JSONValue { .object([:]) }
    func writeCurrentAuth(_ auth: JSONValue) throws {}
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue { .object([:]) }

    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        _ = auth
        return ExtractedAuth(
            accountID: "account-1",
            accessToken: "token",
            email: "test@example.com",
            planType: "pro",
            teamName: nil
        )
    }
}

private struct SettingsTestUsageService: UsageService {
    func fetchUsage(accessToken: String, accountID: String) async throws -> UsageSnapshot {
        _ = accessToken
        _ = accountID
        return UsageSnapshot(
            fetchedAt: 1,
            planType: "pro",
            fiveHour: nil,
            oneWeek: nil,
            credits: nil
        )
    }
}

private struct SettingsStubChatGPTOAuthLoginService: ChatGPTOAuthLoginServiceProtocol {
    func signInWithChatGPT(timeoutSeconds: TimeInterval) async throws -> ChatGPTOAuthTokens {
        _ = timeoutSeconds
        return ChatGPTOAuthTokens(
            accessToken: "token",
            refreshToken: "refresh",
            idToken: "id",
            apiKey: nil
        )
    }
}

private struct SettingsStubCodexCLIService: CodexCLIServiceProtocol {
    func launchApp(workspacePath: String?) throws -> Bool {
        _ = workspacePath
        return true
    }
}

private struct SettingsStubOpencodeAuthSyncService: OpencodeAuthSyncServiceProtocol {
    func syncFromCodexAuth(_ authJSON: JSONValue) throws {
        _ = authJSON
    }
}

private final class SettingsRecordingOpencodeAuthSyncService:
    OpencodeAuthSyncServiceProtocol,
    @unchecked Sendable
{
    private(set) var callCount = 0

    func syncFromCodexAuth(_ authJSON: JSONValue) throws {
        _ = authJSON
        callCount += 1
    }
}

private struct SettingsFixedDateProvider: DateProviding {
    let now: Int64

    func unixSecondsNow() -> Int64 {
        now
    }
}

private struct SettingsStubSub2APIAccountService: Sub2APIAccountServiceProtocol {
    let accounts: [Sub2APIAccountSummary]
    var providerID = "my"
    var accountsByProviderID: [String: [Sub2APIAccountSummary]] = [:]
    var configuredProvider: String? = nil
    var isConnected = true

    func currentDefaultProviderID() -> String {
        providerID
    }

    func configuredProviderID() -> String? {
        configuredProvider ?? providerID
    }

    func isConnectionConfigured() -> Bool {
        isConnected
    }

    func canQueryCurrentDefaultProvider() -> Bool {
        isConnected
    }

    func fetchAccounts(accountIDs: [Int64]?) async throws -> [Sub2APIAccountSummary] {
        guard let accountIDs else { return accounts }
        let ids = Set(accountIDs)
        return accounts.filter { ids.contains($0.id) }
    }

    func fetchAccounts(
        providerID: String,
        accountIDs: [Int64]?
    ) async throws -> [Sub2APIAccountSummary] {
        let providerAccounts = accountsByProviderID[providerID]
            ?? (providerID.caseInsensitiveCompare(self.providerID) == .orderedSame ? accounts : [])
        guard let accountIDs else { return providerAccounts }
        let ids = Set(accountIDs)
        return providerAccounts.filter { ids.contains($0.id) }
    }
}

private final class SettingsStubSub2APISecretStore:
    Sub2APISecretStoreProtocol,
    @unchecked Sendable
{
    private var passwords: [UUID: String] = [:]

    func password(for configurationID: UUID) throws -> String? {
        passwords[configurationID]
    }

    func setPassword(_ password: String, for configurationID: UUID) throws {
        passwords[configurationID] = password
    }

    func removePassword(for configurationID: UUID) throws {
        passwords[configurationID] = nil
    }
}

private final class SettingsStubCodexModelProviderSwitchService:
    CodexModelProviderSwitchServiceProtocol,
    @unchecked Sendable
{
    private(set) var switchedProviderIDs: [String] = []

    func switchProvider(to providerID: String) throws {
        switchedProviderIDs.append(providerID)
    }
}
