import XCTest
@testable import Copool

@MainActor
final class SettingsPageModelTests: XCTestCase {
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

    func testSaveSub2APIProviderPersistsUsernameAndPassword() async throws {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
        let model = SettingsPageModel(
            settingsCoordinator: SettingsCoordinator(
                settingsRepository: settingsRepository,
                launchAtStartupService: SettingsStubLaunchAtStartupService()
            ),
            editorAppService: SettingsStubEditorAppService()
        )
        model.sub2APIProviderDraft = Sub2APIProviderConfiguration(
            isEnabled: true,
            providerID: "my",
            adminBaseURL: "https://sub2.test:6060/api/v1",
            username: "admin@example.com",
            password: "secret",
            allowInsecureTLS: true
        )

        model.saveSub2APIProvider()
        while model.isSavingSub2APIProvider {
            await Task.yield()
        }

        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider,
            model.sub2APIProviderDraft
        )
        XCTAssertEqual(model.notice?.text, L10n.tr("settings.notice.sub2api_saved"))
    }

    func testAccountsPageImportsSub2APIAccountsAndPersistsIDs() async throws {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
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

        XCTAssertEqual(model.sub2APIAccounts, [account])
        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider.importedAccountIDs,
            [42]
        )
        XCTAssertEqual(
            try settingsRepository.loadSettings().sub2APIProvider.cachedAccounts,
            [account]
        )
        guard case .content = model.makeContentPresentation().state else {
            return XCTFail("Sub2api accounts should make the page display content")
        }
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
        settings.sub2APIProvider = Sub2APIProviderConfiguration(
            username: "admin@example.com",
            password: "secret",
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

        XCTAssertEqual(model.sub2APIAccounts, [account])
    }

    func testAccountsPageAsksBeforeAssociatingCurrentProviderWithSub2API() async throws {
        let settingsRepository = TestSettingsRepository(settings: .defaultValue)
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
                isConfirmed: false,
                isConnected: false
            ),
            initialAccounts: []
        )

        await model.importSub2APIAccounts()

        XCTAssertEqual(model.pendingSub2APIProviderConfirmation, "ShareCoder")
        XCTAssertTrue(model.sub2APIAccounts.isEmpty)

        await model.confirmSub2APIProviderAndImport(providerID: "ShareCoder")

        XCTAssertEqual(model.sub2APIAccounts, [account])
        XCTAssertTrue(
            try settingsRepository.loadSettings().sub2APIProvider.confirms(providerID: "ShareCoder")
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
    private var store = AccountsStore()

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

private struct SettingsFixedDateProvider: DateProviding {
    let now: Int64

    func unixSecondsNow() -> Int64 {
        now
    }
}

private struct SettingsStubSub2APIAccountService: Sub2APIAccountServiceProtocol {
    let accounts: [Sub2APIAccountSummary]
    var providerID = "my"
    var isConfirmed = true
    var isConnected = true

    func currentDefaultProviderID() -> String {
        providerID
    }

    func isConnectionConfigured() -> Bool {
        isConnected
    }

    func isCurrentDefaultProviderConfirmed() -> Bool {
        isConfirmed
    }

    func fetchAccounts(accountIDs: [Int64]?) async throws -> [Sub2APIAccountSummary] {
        guard let accountIDs else { return accounts }
        let ids = Set(accountIDs)
        return accounts.filter { ids.contains($0.id) }
    }
}
