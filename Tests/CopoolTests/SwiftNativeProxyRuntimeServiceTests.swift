import XCTest
@testable import Copool

final class SwiftNativeProxyRuntimeServiceTests: XCTestCase {
    func testLocalCandidatesKeepIndependentAccountProxies() async throws {
        var first = makeStoredAccount(id: "a", label: "A", accountID: "a", addedAt: 1)
        var second = makeStoredAccount(id: "b", label: "B", accountID: "b", addedAt: 2)
        first.proxyURL = "http://127.0.0.1:8080"
        second.proxyURL = "socks5://127.0.0.1:1080"
        let runtime = makeRuntime(store: AccountsStore(accounts: [first, second]))
        let candidates = try await runtime.withIsolation { try $0.loadCandidates() }
        XCTAssertEqual(candidates.map(\.proxyURL), [first.proxyURL, second.proxyURL])
    }

    func testLoadCandidatesPrefersCurrentSelectionWhenUsageIsUnavailable() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ],
                currentAccountID: "b",
                currentSelection: CurrentAccountSelection(
                    cardID: "acct-b",
                    selectedAt: 2,
                    sourceDeviceID: "macos-local"
                )
            )
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-b", "acct-a"])
    }

    func testLoadCandidatesPrefersCurrentCardIDOverStaleSelection() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ],
                currentAccountID: "b",
                currentSelection: CurrentAccountSelection(
                    cardID: "acct-a",
                    selectedAt: 2,
                    sourceDeviceID: "macos-local"
                )
            )
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-b", "acct-a"])
    }

    func testLoadCandidatesFallsBackToCurrentCardWhenSelectionIdentityIsAmbiguous() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "shared-account", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "shared-account", addedAt: 2)
                ],
                currentAccountID: "b",
                currentSelection: CurrentAccountSelection(
                    cardID: "shared-account",
                    selectedAt: 2,
                    sourceDeviceID: "macos-local"
                )
            )
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.map(\.id), ["b", "a"])
    }

    func testLoadCandidatesUsesAddedAtAsStableTieBreakerForEqualScores() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "later", label: "Later", accountID: "acct-later", addedAt: 20),
                    makeStoredAccount(id: "earlier", label: "Earlier", accountID: "acct-earlier", addedAt: 10)
                ]
            )
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-earlier", "acct-later"])
    }

    func testLoadCandidatesIncludesImportedSub2APIProviderUsingLoginShellFallback() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let configPath = tempDir.appendingPathComponent("config.toml")
        try """
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test/v1"
        wire_api = "responses"
        env_key = "MY_SUB2API_API_KEY"
        requires_openai_auth = false
        """.write(to: configPath, atomically: true, encoding: .utf8)

        let importedAccount = Sub2APIAccountSummary(
            id: 42,
            name: "remote-account",
            email: "remote@example.com",
            accountID: "remote-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "my"
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "admin@example.com",
                    allowInsecureTLS: true,
                    proxyURL: "socks5://127.0.0.1:1080",
                    importedAccountIDs: [42],
                    cachedAccounts: [importedAccount]
                )
            ]
        )
        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: configPath,
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )
        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: InMemoryAccountsStoreRepository(store: AccountsStore()),
            settingsRepository: MockSettingsRepository(settings: settings),
            authRepository: MockAuthRepository(),
            environment: [:],
            providerEnvironmentFallback: { environmentKey in
                environmentKey == "MY_SUB2API_API_KEY" ? "provider-api-key" : nil
            }
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.count, 1)
        XCTAssertEqual(candidates[0].label, "my")
        XCTAssertEqual(candidates[0].accessToken, "provider-api-key")
        XCTAssertTrue(candidates[0].isPreferredCurrent)
        XCTAssertTrue(candidates[0].allowInsecureTLS)
        XCTAssertEqual(candidates[0].proxyURL, "socks5://127.0.0.1:1080")
        XCTAssertEqual(
            candidates[0].route,
            .modelProvider(providerID: "my", baseURL: "https://sub2.test/v1")
        )
    }

    func testImportedSub2APIAccountOverridesCreateIndependentProxyRoutes() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let configPath = tempDir.appendingPathComponent("config.toml")
        try """
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test/v1"
        wire_api = "responses"
        env_key = "MY_SUB2API_API_KEY"
        requires_openai_auth = false
        """.write(to: configPath, atomically: true, encoding: .utf8)

        let importedAccount = Sub2APIAccountSummary(
            id: 42,
            name: "remote-account",
            email: "remote@example.com",
            accountID: "remote-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "my"
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "admin@example.com",
                    allowInsecureTLS: true,
                    proxyURL: "socks5://127.0.0.1:1080",
                    accountProxyURLs: ["43": "http://127.0.0.1:8080"],
                    importedAccountIDs: [42, 43],
                    cachedAccounts: [importedAccount]
                )
            ]
        )
        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: configPath,
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )
        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: InMemoryAccountsStoreRepository(store: AccountsStore()),
            settingsRepository: MockSettingsRepository(settings: settings),
            authRepository: MockAuthRepository(),
            environment: [:],
            providerEnvironmentFallback: { environmentKey in
                environmentKey == "MY_SUB2API_API_KEY" ? "provider-api-key" : nil
            }
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.count, 2)
        XCTAssertEqual(Set(candidates.map(\.proxyURL)), ["socks5://127.0.0.1:1080", "http://127.0.0.1:8080"])
        XCTAssertEqual(Set(candidates.map(\.id)).count, 2)
    }

    func testLoadCandidatesIncludesSub2APIProviderFromProfileConfig() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let configPath = tempDir.appendingPathComponent("config.toml")
        try "model_provider = \"openai\"\n".write(
            to: configPath,
            atomically: true,
            encoding: .utf8
        )
        try """
        model_provider = "profile-provider"

        [model_providers.profile-provider]
        base_url = "https://profile-sub2.test/v1"
        wire_api = "responses"
        env_key = "PROFILE_SUB2API_KEY"
        requires_openai_auth = false
        """.write(
            to: tempDir.appendingPathComponent("work.config.toml"),
            atomically: true,
            encoding: .utf8
        )

        let importedAccount = Sub2APIAccountSummary(
            id: 42,
            name: "remote-account",
            email: "remote@example.com",
            accountID: "remote-account",
            accountType: "oauth",
            status: "active",
            planType: "pro",
            usage: nil,
            usageError: nil,
            providerID: "profile-provider"
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "profile-provider",
                    username: "admin@example.com",
                    importedAccountIDs: [42],
                    cachedAccounts: [importedAccount]
                )
            ]
        )
        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: configPath,
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )
        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: InMemoryAccountsStoreRepository(store: AccountsStore()),
            settingsRepository: MockSettingsRepository(settings: settings),
            authRepository: MockAuthRepository(),
            environment: ["PROFILE_SUB2API_KEY": "provider-api-key"],
            providerEnvironmentFallback: { _ in nil }
        )

        let candidates = try await runtime.withIsolation { runtime in
            try runtime.loadCandidates()
        }

        XCTAssertEqual(candidates.count, 1)
        XCTAssertEqual(candidates[0].label, "profile-provider")
        XCTAssertEqual(candidates[0].accessToken, "provider-api-key")
        XCTAssertFalse(candidates[0].isPreferredCurrent)
        XCTAssertEqual(
            candidates[0].route,
            .modelProvider(
                providerID: "profile-provider",
                baseURL: "https://profile-sub2.test/v1"
            )
        )
    }

    func testModelProviderCandidateUsesProviderResponsesEndpointWithoutChatGPTAccountHeader() async throws {
        let runtime = makeRuntime(store: AccountsStore())
        let candidate = ProxyCandidate(
            id: "sub2api-provider:my",
            label: "my",
            accountID: "sub2api-provider:my",
            accountKey: "sub2api-provider:my",
            accessToken: "provider-api-key",
            authJSON: .null,
            addedAt: 42,
            isPreferredCurrent: true,
            oneWeekUsed: nil,
            fiveHourUsed: nil,
            route: .modelProvider(providerID: "my", baseURL: "https://sub2.test/v1")
        )

        let request = try await runtime.withIsolation { runtime in
            try runtime.makeUpstreamRequest(
                payload: ["model": "gpt-5.4", "input": []],
                candidate: candidate,
                downstreamHeaders: [:]
            )
        }

        XCTAssertEqual(request.url?.absoluteString, "https://sub2.test/v1/responses")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer provider-api-key")
        XCTAssertNil(request.value(forHTTPHeaderField: "ChatGPT-Account-Id"))
    }

    func testParsesCodexVersionOnlyFromRecognizedUserAgentProducts() {
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.parseCodexVersion(
                fromUserAgent: "codex_exec/7.8.9 (Mac OS 26.0.1; arm64) Apple_Terminal/464"
            ),
            "7.8.9"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.parseCodexVersion(
                fromUserAgent: "codex_cli_rs/6.7.8 (Mac OS 26.0.1; arm64)"
            ),
            "6.7.8"
        )
        XCTAssertNil(
            SwiftNativeProxyRuntimeService.parseCodexVersion(
                fromUserAgent: "openai-python/1.101.0 Python/3.13"
            )
        )
        XCTAssertNil(
            SwiftNativeProxyRuntimeService.parseCodexVersion(
                fromUserAgent: "codex_exec/not-a-version"
            )
        )
    }

    func testUpstreamVersionPrefersExplicitHeaderThenCodexUserAgentThenFallback() async throws {
        let runtime = makeRuntime(store: AccountsStore())
        let candidate = ProxyCandidate(
            id: "sub2api-provider:my",
            label: "my",
            accountID: "sub2api-provider:my",
            accountKey: "sub2api-provider:my",
            accessToken: "provider-api-key",
            authJSON: .null,
            addedAt: 42,
            isPreferredCurrent: true,
            oneWeekUsed: nil,
            fiveHourUsed: nil,
            route: .modelProvider(providerID: "my", baseURL: "https://sub2.test/v1")
        )

        let versions = try await runtime.withIsolation { runtime in
            let explicit = try runtime.makeUpstreamRequest(
                payload: ["model": "gpt-5.4", "input": []],
                candidate: candidate,
                downstreamHeaders: [
                    "version": "9.9.9",
                    "user-agent": "codex_exec/7.8.9 (Mac OS 26.0.1; arm64)",
                ]
            )
            let inferred = try runtime.makeUpstreamRequest(
                payload: ["model": "gpt-5.4", "input": []],
                candidate: candidate,
                downstreamHeaders: [
                    "user-agent": "codex_exec/7.8.9 (Mac OS 26.0.1; arm64)",
                ]
            )
            let fallback = try runtime.makeUpstreamRequest(
                payload: ["model": "gpt-5.4", "input": []],
                candidate: candidate,
                downstreamHeaders: ["user-agent": "openai-python/1.101.0"]
            )
            return (
                explicit.value(forHTTPHeaderField: "Version"),
                inferred.value(forHTTPHeaderField: "Version"),
                fallback.value(forHTTPHeaderField: "Version")
            )
        }

        XCTAssertEqual(versions.0, "9.9.9")
        XCTAssertEqual(versions.1, "7.8.9")
        XCTAssertEqual(versions.2, SwiftNativeProxyRuntimeService.defaultCodexClientVersion)
    }

    func testRecordSuccessfulProviderCandidateUsesProviderSwitchHandler() async throws {
        let recorder = ProviderSwitchRecorder()
        let runtime = makeRuntime(
            storeRepository: InMemoryAccountsStoreRepository(store: AccountsStore()),
            switchModelProvider: { providerID in
                recorder.record(providerID)
            }
        )
        let candidate = ProxyCandidate(
            id: "sub2api-provider:my",
            label: "my",
            accountID: "sub2api-provider:my",
            accountKey: "sub2api-provider:my",
            accessToken: "provider-api-key",
            authJSON: .null,
            addedAt: 42,
            isPreferredCurrent: false,
            oneWeekUsed: nil,
            fiveHourUsed: nil,
            route: .modelProvider(providerID: "my", baseURL: "https://sub2.test/v1")
        )

        try await runtime.recordSuccessfulCandidate(candidate)

        XCTAssertEqual(recorder.providerIDs, ["my"])
    }

    func testCurrentCandidatesPrefersStickyAccountAfterSuccessfulSelection() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ]
            )
        )

        try await runtime.recordSuccessfulCandidate(
            ProxyCandidate(
                id: "b",
                label: "Account B",
                accountID: "acct-b",
                accountKey: "acct-b|acct-b",
                accessToken: "token-acct-b",
                authJSON: .object([:]),
                addedAt: 2,
                isPreferredCurrent: false,
                oneWeekUsed: nil,
                fiveHourUsed: nil
            )
        )
        let candidates = try await runtime.withIsolation { runtime in
            return try runtime.currentCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-b", "acct-a"])
    }

    func testCurrentCandidatesPrefersManualSelectionOverStickyAccount() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ],
                currentAccountID: "b",
                currentSelection: CurrentAccountSelection(
                    cardID: "acct-b",
                    selectedAt: 2,
                    sourceDeviceID: "macos-local"
                )
            )
        )

        let candidates = try await runtime.withIsolation { runtime in
            runtime.stickyAccountID = "acct-a"
            return try runtime.currentCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-b", "acct-a"])
    }

    func testCurrentCandidatesSkipsAccountInCooldownWindow() async throws {
        let runtime = makeRuntime(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ]
            ),
            dateProvider: FixedDateProvider(unixSeconds: 100, unixMilliseconds: 100_000)
        )

        let candidates = try await runtime.withIsolation { runtime in
            runtime.markCooldown(for: "acct-a", category: .rateLimited)
            return try runtime.currentCandidates()
        }

        XCTAssertEqual(candidates.map(\.accountID), ["acct-b"])
    }

    func testRecordSuccessfulCandidateCallsStoreChangeHandler() async throws {
        let callback = StoreChangeCallback()
        let runtime = makeRuntime(
            storeRepository: InMemoryAccountsStoreRepository(
                store: AccountsStore(
                    accounts: [
                        makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1)
                    ]
                )
            ),
            onAccountsStoreChanged: {
                callback.markCalled()
            }
        )
        let candidate = ProxyCandidate(
            id: "a",
            label: "Account A",
            accountID: "acct-a",
            accountKey: "acct-a|acct-a",
            accessToken: "token-acct-a",
            authJSON: .object([
                "tokens": .object([
                    "access_token": .string("token-acct-a"),
                    "account_id": .string("acct-a")
                ])
            ]),
            addedAt: 1,
            isPreferredCurrent: false,
            oneWeekUsed: nil,
            fiveHourUsed: nil
        )

        try await runtime.recordSuccessfulCandidate(candidate)

        let callbackCount = callback.readCallCount()
        XCTAssertEqual(callbackCount, 1)
    }

    func testRecordSuccessfulCandidateUsesSuccessfulCandidateAsCurrentSelection() async throws {
        let repository = InMemoryAccountsStoreRepository(
            store: AccountsStore(
                accounts: [
                    makeStoredAccount(id: "a", label: "Account A", accountID: "acct-a", addedAt: 1),
                    makeStoredAccount(id: "b", label: "Account B", accountID: "acct-b", addedAt: 2)
                ],
                currentSelection: CurrentAccountSelection(
                    cardID: "acct-b",
                    selectedAt: 2,
                    sourceDeviceID: "macos-local"
                )
            )
        )
        let runtime = makeRuntime(
            storeRepository: repository,
            authRepository: RecordingAuthRepository()
        )
        let candidate = ProxyCandidate(
            id: "a",
            label: "Account A",
            accountID: "acct-a",
            accountKey: "acct-a|acct-a",
            accessToken: "token-acct-a",
            authJSON: .object([
                "tokens": .object([
                    "access_token": .string("token-acct-a"),
                    "account_id": .string("acct-a")
                ])
            ]),
            addedAt: 1,
            isPreferredCurrent: false,
            oneWeekUsed: nil,
            fiveHourUsed: nil
        )

        try await runtime.recordSuccessfulCandidate(candidate)

        XCTAssertEqual(repository.store.currentSelection?.cardID, "acct-a")
    }

    func testNormalizesReasoningSummaryForUpstream() {
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningSummaryForUpstream("none"),
            "auto"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningSummaryForUpstream("  NONE "),
            "auto"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningSummaryForUpstream(nil),
            "auto"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningSummaryForUpstream("concise"),
            "concise"
        )
    }

    func testNormalizesReasoningEffortForUpstream() {
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream(
                "none",
                upstreamModel: "gpt-5.1-codex-max"
            ),
            "medium"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream("HIGH"),
            "high"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream(
                "xhigh",
                upstreamModel: "gpt-5.3-codex"
            ),
            "xhigh"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream(
                "minimal",
                upstreamModel: "gpt-5.3-codex"
            ),
            "medium"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream(
                "none",
                upstreamModel: "gpt-4.1"
            ),
            "none"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream("unexpected"),
            "none"
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.normalizedReasoningEffortForUpstream(nil),
            "none"
        )
    }

    func testHostAPIOnlyDisablesCurrentAuthSyncAfterSuccessfulProxyResponse() {
        XCTAssertFalse(
            SwiftNativeProxyRuntimeService.shouldSyncCurrentAuthOnSuccessfulProxyResponse(
                localProxyHostAPIOnly: true
            )
        )
        XCTAssertTrue(
            SwiftNativeProxyRuntimeService.shouldSyncCurrentAuthOnSuccessfulProxyResponse(
                localProxyHostAPIOnly: false
            )
        )
    }

    func testHostAPIOnlyRecordsActiveCandidateWithoutSwitchingSelection() async throws {
        let account = makeStoredAccount(
            id: "a",
            label: "Account A",
            accountID: "acct-a",
            addedAt: 1
        )
        let repository = InMemoryAccountsStoreRepository(
            store: AccountsStore(accounts: [account])
        )
        var settings = AppSettings.defaultValue
        settings.localProxyHostAPIOnly = true
        let switchRecorder = AccountSwitchRecorder()
        let runtime = makeRuntime(
            storeRepository: repository,
            settingsRepository: MockSettingsRepository(settings: settings),
            switchAccount: { cardID in
                switchRecorder.record(cardID)
            }
        )
        let candidate = ProxyCandidate(
            id: account.id,
            label: account.label,
            accountID: account.accountID,
            accountKey: account.accountKey,
            accessToken: "token-acct-a",
            authJSON: account.authJSON,
            addedAt: account.addedAt,
            isPreferredCurrent: false,
            oneWeekUsed: nil,
            fiveHourUsed: nil
        )

        try await runtime.recordSuccessfulCandidate(candidate)
        let status = await runtime.status()

        XCTAssertEqual(switchRecorder.cardIDs, [])
        XCTAssertNil(repository.store.currentAccountID)
        XCTAssertEqual(status.activeAccountID, "acct-a")
        XCTAssertEqual(status.activeAccountLabel, "Account A")
    }

    func testRepeatedSuccessForCurrentAccountDoesNotResyncSelection() async throws {
        let account = makeStoredAccount(
            id: "a",
            label: "Account A",
            accountID: "acct-a",
            addedAt: 1
        )
        let repository = InMemoryAccountsStoreRepository(
            store: AccountsStore(accounts: [account], currentAccountID: account.id)
        )
        let authRepository = RecordingAuthRepository(currentAuth: account.authJSON)
        let switchRecorder = AccountSwitchRecorder()
        let runtime = makeRuntime(
            storeRepository: repository,
            authRepository: authRepository,
            switchAccount: { cardID in
                switchRecorder.record(cardID)
            }
        )
        let candidate = ProxyCandidate(
            id: account.id,
            label: account.label,
            accountID: account.accountID,
            accountKey: account.accountKey,
            accessToken: "token-acct-a",
            authJSON: account.authJSON,
            addedAt: account.addedAt,
            isPreferredCurrent: true,
            oneWeekUsed: nil,
            fiveHourUsed: nil
        )

        try await runtime.recordSuccessfulCandidate(candidate)
        try await runtime.recordSuccessfulCandidate(candidate)

        XCTAssertEqual(switchRecorder.cardIDs, [])
        XCTAssertEqual(authRepository.writeCurrentAuthCallCount, 0)
    }

    func testRepeatedSuccessForCurrentProviderDoesNotResyncProvider() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }
        let configPath = tempDir.appendingPathComponent("config.toml")
        try """
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test/v1"
        """.write(to: configPath, atomically: true, encoding: .utf8)

        let providerRecorder = ProviderSwitchRecorder()
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: tempDir,
                accountStorePath: tempDir.appendingPathComponent("accounts.json"),
                settingsStorePath: tempDir.appendingPathComponent("settings.json"),
                codexAuthPath: tempDir.appendingPathComponent("auth.json"),
                codexConfigPath: configPath,
                proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
                proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
                cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
            ),
            storeRepository: InMemoryAccountsStoreRepository(store: AccountsStore()),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository(),
            switchModelProvider: { providerID in
                providerRecorder.record(providerID)
            },
            environment: [:],
            providerEnvironmentFallback: { _ in nil }
        )
        let candidate = ProxyCandidate(
            id: "sub2api-provider:my",
            label: "my",
            accountID: "sub2api-provider:my",
            accountKey: "sub2api-provider:my",
            accessToken: "provider-api-key",
            authJSON: .null,
            addedAt: 42,
            isPreferredCurrent: true,
            oneWeekUsed: nil,
            fiveHourUsed: nil,
            route: .modelProvider(providerID: "my", baseURL: "https://sub2.test/v1")
        )

        try await runtime.recordSuccessfulCandidate(candidate)
        try await runtime.recordSuccessfulCandidate(candidate)

        XCTAssertEqual(providerRecorder.providerIDs, [])
    }

    func testResolvesUpstreamRouteFamilyByModel() {
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5.4"),
            .codex
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5-4"),
            .codex
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5-codex-mini"),
            .codex
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5"),
            .codex
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5-mini"),
            .codex
        )
        XCTAssertEqual(
            SwiftNativeProxyRuntimeService.resolveUpstreamRouteFamily(forUpstreamModel: "gpt-5.2"),
            .codex
        )
    }

    func testMapsDisplayModelNamesToUpstream() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let mapped = try await runtime.withIsolation { runtime in
            (
                try runtime.mapClientModelToUpstream("GPT-5.4"),
                try runtime.mapClientModelToUpstream("GPT-5.4-Mini"),
                try runtime.mapClientModelToUpstream("GPT-5.3-Codex")
            )
        }

        XCTAssertEqual(mapped.0, "gpt-5.4")
        XCTAssertEqual(mapped.1, "gpt-5.4-mini")
        XCTAssertEqual(mapped.2, "gpt-5.3-codex")
    }

    func testMapsDisplayModelAliasNamesToUpstream() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let mapped = try await runtime.withIsolation { runtime in
            (
                try runtime.mapClientModelToUpstream("GPT-5.4-Low"),
                try runtime.mapClientModelToUpstream("GPT-5.4-High"),
                try runtime.mapClientModelToUpstream("GPT-5.4-Mini-High"),
                try runtime.mapClientModelToUpstream("GPT-5.4-Mini-xHigh"),
                try runtime.mapClientModelToUpstream("GPT-5.3-Codex-Medium")
            )
        }

        XCTAssertEqual(mapped.0, "gpt-5.4")
        XCTAssertEqual(mapped.1, "gpt-5.4")
        XCTAssertEqual(mapped.2, "gpt-5.4-mini")
        XCTAssertEqual(mapped.3, "gpt-5.4-mini")
        XCTAssertEqual(mapped.4, "gpt-5.3-codex")
    }

    func testNormalizesUpstreamModelsForClientDisplay() async {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let normalized = await runtime.withIsolation { runtime in
            (
                runtime.normalizeModelForClient("gpt-5"),
                runtime.normalizeModelForClient("gpt-5.3-codex"),
                runtime.normalizeModelForClient("gpt-5-4"),
                runtime.normalizeModelForClient("gpt-5-4-mini"),
                runtime.normalizeModelForClient("gpt-5-4-xhigh"),
                runtime.normalizeModelForClient("gpt5.4-2026-03-09")
            )
        }

        XCTAssertEqual(normalized.0, "GPT-5")
        XCTAssertEqual(normalized.1, "GPT-5.3-Codex")
        XCTAssertEqual(normalized.2, "GPT-5.4")
        XCTAssertEqual(normalized.3, "GPT-5.4-Mini")
        XCTAssertEqual(normalized.4, "GPT-5.4-xHigh")
        XCTAssertEqual(normalized.5, "GPT-5.4-2026-03-09")
    }

    func testClientVisibleModelsIncludeReasoningAliasNames() {
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.5"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.5-High"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.4-Low"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.4-High"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.4-Mini-High"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.4-Mini-xHigh"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.3-Codex-xHigh"))
        XCTAssertTrue(SwiftNativeProxyRuntimeService.clientVisibleModels.contains("GPT-5.3-Codex-Medium"))
    }

    func testResolvesUpstreamBaseURLForBothRouteFamilies() {
        let codexFromOrigin = SwiftNativeProxyRuntimeService.resolveUpstreamBaseURL(
            configuredBaseURL: "https://chatgpt.com",
            routeFamily: .codex
        )
        XCTAssertEqual(codexFromOrigin, "https://chatgpt.com/backend-api/codex")

        let generalFromOrigin = SwiftNativeProxyRuntimeService.resolveUpstreamBaseURL(
            configuredBaseURL: "https://chatgpt.com",
            routeFamily: .general
        )
        XCTAssertEqual(generalFromOrigin, "https://chatgpt.com/backend-api")

        let codexFromResponses = SwiftNativeProxyRuntimeService.resolveUpstreamBaseURL(
            configuredBaseURL: "https://chatgpt.com/backend-api/codex/responses",
            routeFamily: .codex
        )
        XCTAssertEqual(codexFromResponses, "https://chatgpt.com/backend-api/codex")

        let generalFromResponses = SwiftNativeProxyRuntimeService.resolveUpstreamBaseURL(
            configuredBaseURL: "https://chatgpt.com/backend-api/responses",
            routeFamily: .general
        )
        XCTAssertEqual(generalFromResponses, "https://chatgpt.com/backend-api")
    }

    func testHealthAndModelsEndpoints() async throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let storeRepo = MockStoreRepository()
        let authRepo = MockAuthRepository()
        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: storeRepo,
            settingsRepository: MockSettingsRepository(),
            authRepository: authRepo
        )

        let port = Int.random(in: 21000...29000)
        let started = try await runtime.start(preferredPort: port)
        defer {
            Task { _ = await runtime.stop() }
        }

        XCTAssertTrue(started.running)
        XCTAssertEqual(started.port, port)
        XCTAssertNotNil(started.apiKey)
        XCTAssertTrue(started.apiKey?.hasPrefix("sk-") == true)

        let healthURL = URL(string: "http://127.0.0.1:\(port)/health")!
        let (healthData, healthResponse) = try await URLSession.shared.data(from: healthURL)
        XCTAssertEqual((healthResponse as? HTTPURLResponse)?.statusCode, 200)
        XCTAssertEqual(try parseJSON(healthData)["ok"] as? Bool, true)

        let modelsURL = URL(string: "http://127.0.0.1:\(port)/v1/models")!
        var modelsRequest = URLRequest(url: modelsURL)
        modelsRequest.setValue("Bearer \(started.apiKey ?? "")", forHTTPHeaderField: "Authorization")
        let (modelsData, modelsResponse) = try await URLSession.shared.data(for: modelsRequest)
        XCTAssertEqual((modelsResponse as? HTTPURLResponse)?.statusCode, 200)

        let modelsJSON = try parseJSON(modelsData)
        let modelItems = modelsJSON["data"] as? [[String: Any]]
        XCTAssertNotNil(modelItems)
        XCTAssertTrue((modelItems?.count ?? 0) > 0)
        let ids = (modelItems ?? []).compactMap { $0["id"] as? String }
        XCTAssertEqual(ids, SwiftNativeProxyRuntimeService.clientVisibleModels)

        var modelsByAPIKeyHeader = URLRequest(url: modelsURL)
        modelsByAPIKeyHeader.setValue(started.apiKey ?? "", forHTTPHeaderField: "x-api-key")
        let (_, modelsByAPIKeyHeaderResponse) = try await URLSession.shared.data(for: modelsByAPIKeyHeader)
        XCTAssertEqual((modelsByAPIKeyHeaderResponse as? HTTPURLResponse)?.statusCode, 200)
    }

    func testStartKeepsLegacyPersistedAPIKey() async throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        try FileManager.default.createDirectory(
            at: paths.proxyDaemonDataDirectory,
            withIntermediateDirectories: true
        )
        let legacyKey = "legacy-proxy-key"
        try legacyKey.write(to: paths.proxyDaemonKeyPath, atomically: true, encoding: .utf8)

        let account = StoredAccount(
            id: "acct-1",
            label: "Primary",
            email: nil,
            accountID: "account-1",
            planType: nil,
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([
                "tokens": .object([
                    "access_token": .string("token"),
                    "id_token": .string("id-token"),
                    "account_id": .string("account-1")
                ])
            ]),
            addedAt: 1,
            updatedAt: 1,
            usage: nil,
            usageError: nil
        )
        let storeRepository = CountingStoreRepository(store: AccountsStore(accounts: [account]))
        let authRepository = CountingAuthRepository()
        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: storeRepository,
            settingsRepository: MockSettingsRepository(),
            authRepository: authRepository
        )

        let port = Int.random(in: 21000...29000)
        let started = try await runtime.start(preferredPort: port)
        defer {
            Task { _ = await runtime.stop() }
        }

        XCTAssertEqual(started.apiKey, legacyKey)

        let firstStatus = await runtime.status()
        let secondStatus = await runtime.status()

        XCTAssertEqual(firstStatus.availableAccounts, 1)
        XCTAssertEqual(secondStatus.availableAccounts, 1)
        XCTAssertEqual(storeRepository.loadStoreCallCount, 1)
        XCTAssertEqual(authRepository.extractAuthCallCount, 1)
    }

    func testResponsesRejectsMissingModel() async throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: tempDir.appendingPathComponent("accounts.json"),
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let runtime = SwiftNativeProxyRuntimeService(
            paths: paths,
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let port = Int.random(in: 30000...36000)
        let started = try await runtime.start(preferredPort: port)
        defer {
            Task { _ = await runtime.stop() }
        }

        let url = URL(string: "http://127.0.0.1:\(port)/v1/responses")!
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = try JSONSerialization.data(withJSONObject: ["input": "hello"])
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(started.apiKey ?? "")", forHTTPHeaderField: "Authorization")

        let (data, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 400)

        let json = try parseJSON(data)
        let error = json["error"] as? [String: Any]
        XCTAssertNotNil(error)
    }

    func testResponsesNormalizesStringInputToMessageArray() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let normalized = try await runtime.withIsolation { runtime in
            snapshot(
                from: try runtime.normalizeResponsesRequest([
                "model": "gpt-5.4",
                "input": "reply with exactly OK",
                "stream": false
                ])
            )
        }

        XCTAssertEqual(normalized.downstreamStream, false)
        XCTAssertEqual(normalized.model, "gpt-5.4")
        XCTAssertEqual(normalized.stream, true)
        XCTAssertEqual(normalized.input.count, 1)
        XCTAssertEqual(normalized.input[0].type, "message")
        XCTAssertEqual(normalized.input[0].role, "user")
        XCTAssertEqual(normalized.input[0].content.count, 1)
        XCTAssertEqual(normalized.input[0].content[0].type, "input_text")
        XCTAssertEqual(normalized.input[0].content[0].text, "reply with exactly OK")
    }

    func testResponsesNormalizationDropsUnsupportedForwardingFields() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let retainedUnsupportedKeys = try await runtime.withIsolation { runtime in
            let normalized = try runtime.normalizeResponsesRequest([
                "model": "gpt-5.4",
                "input": [[
                    "role": "user",
                    "content": [[
                        "type": "input_text",
                        "text": "hello"
                    ]]
                ]],
                "prompt_cache_key": "factory-droid",
                "prompt_cache_retention": "24h",
                "safety_identifier": "user-123",
                "service_tier": "auto"
            ])
            return [
                "prompt_cache_key",
                "prompt_cache_retention",
                "safety_identifier",
                "service_tier"
            ].filter { normalized.payload[$0] != nil }
        }

        XCTAssertEqual(retainedUnsupportedKeys, [])
    }

    func testResponsesNormalizationDropsMaxOutputTokensAndTemperature() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let retainedKeys = try await runtime.withIsolation { runtime in
            let normalized = try runtime.normalizeResponsesRequest([
                "model": "gpt-5.4",
                "input": "hello",
                "max_output_tokens": 32,
                "temperature": 0.1
            ])
            return ["max_output_tokens", "temperature"].filter { normalized.payload[$0] != nil }
        }

        XCTAssertEqual(retainedKeys, [])
    }

    func testResponsesNormalizationInjectsReasoningEffortFromModelAlias() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let normalizedReasoningEffort = try await runtime.withIsolation { runtime in
            let normalized = try runtime.normalizeResponsesRequest([
                "model": "GPT-5.4-High",
                "input": "hello"
            ])
            return (normalized.payload["reasoning"] as? [String: Any])?["effort"] as? String
        }

        let normalizedReasoningSummary = try await runtime.withIsolation { runtime in
            let normalized = try runtime.normalizeResponsesRequest([
                "model": "GPT-5.4-High",
                "input": "hello"
            ])
            return (normalized.payload["reasoning"] as? [String: Any])?["summary"] as? String
        }

        XCTAssertEqual(normalizedReasoningEffort, "high")
        XCTAssertEqual(normalizedReasoningSummary, "auto")
    }

    func testResponsesNormalizationKeepsExplicitReasoningEffortOverAliasDefault() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let normalizedReasoningEffort = try await runtime.withIsolation { runtime in
            let normalized = try runtime.normalizeResponsesRequest([
                "model": "GPT-5.4-High",
                "input": "hello",
                "reasoning": [
                    "effort": "low"
                ]
            ])
            return (normalized.payload["reasoning"] as? [String: Any])?["effort"] as? String
        }

        XCTAssertEqual(normalizedReasoningEffort, "low")
    }

    func testChatConversionInjectsReasoningEffortFromModelAlias() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let normalizedReasoningEffort = try await runtime.withIsolation { runtime in
            let normalized = try runtime.convertChatRequestToResponses([
                "model": "GPT-5.3-Codex-High",
                "messages": [
                    [
                        "role": "user",
                        "content": "hello"
                    ]
                ]
            ])
            return (normalized.payload["reasoning"] as? [String: Any])?["effort"] as? String
        }

        XCTAssertEqual(normalizedReasoningEffort, "high")
    }

    func testChatConversionMapsJSONResponseFormatToTextFormatType() async throws {
        let runtime = SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: URL(fileURLWithPath: "/tmp"),
                accountStorePath: URL(fileURLWithPath: "/tmp/accounts.json"),
                settingsStorePath: URL(fileURLWithPath: "/tmp/settings.json"),
                codexAuthPath: URL(fileURLWithPath: "/tmp/auth.json"),
                codexConfigPath: URL(fileURLWithPath: "/tmp/config.toml"),
                proxyDaemonDataDirectory: URL(fileURLWithPath: "/tmp/proxyd", isDirectory: true),
                proxyDaemonKeyPath: URL(fileURLWithPath: "/tmp/proxyd/api-proxy.key"),
                cloudflaredLogDirectory: URL(fileURLWithPath: "/tmp/cloudflared-logs", isDirectory: true)
            ),
            storeRepository: MockStoreRepository(),
            settingsRepository: MockSettingsRepository(),
            authRepository: MockAuthRepository()
        )

        let mappedType = try await runtime.withIsolation { runtime in
            let normalized = try runtime.convertChatRequestToResponses([
                "model": "GPT-5.4",
                "messages": [
                    [
                        "role": "user",
                        "content": "return valid json"
                    ]
                ],
                "response_format": [
                    "type": "json_object"
                ]
            ])
            return ((normalized.payload["text"] as? [String: Any])?["format"] as? [String: Any])?["type"] as? String
        }

        XCTAssertEqual(mappedType, "json_object")
    }

    func testStreamingChatTranslatorEmitsChunksIncrementally() async throws {
        let runtime = makeRuntime(store: AccountsStore())
        let decoder = await runtime.withIsolation { runtime in
            runtime.makeChatCompletionsSSEStreamDecoder(fallbackModel: "gpt-5")
        }

        let firstChunkCount = try await runtime.withIsolation { runtime in
            let chunks = try runtime.consumeChatCompletionsSSEStreamChunk(
                decoder,
                data: Data("""
                event: response.created
                data: {"type":"response.created","response":{"id":"resp_123","created_at":1,"model":"gpt-5"}}

                event: response.output_text.delta
                data: {"type":"response.output_text.delta","delta":"Hel
                """.utf8),
                isFinal: false
            )
            return chunks.count
        }
        XCTAssertEqual(firstChunkCount, 0)

        let secondSnapshot = try await runtime.withIsolation { runtime in
            let chunks = try runtime.consumeChatCompletionsSSEStreamChunk(
                decoder,
                data: Data("""
                lo"}

                event: response.completed
                data: {"type":"response.completed","response":{"id":"resp_123","created_at":1,"model":"gpt-5","status":"completed"}}

                """.utf8),
                isFinal: true
            )
            let firstContent = ((chunks[0]["choices"] as? [[String: Any]])?.first?["delta"] as? [String: Any])?["content"] as? String
            let finishReason = ((chunks[1]["choices"] as? [[String: Any]])?.first?["finish_reason"] as? String)
            return StreamedChatChunkSnapshot(
                chunkCount: chunks.count,
                firstContent: firstContent,
                finishReason: finishReason
            )
        }

        XCTAssertEqual(secondSnapshot.chunkCount, 2)
        XCTAssertEqual(secondSnapshot.firstContent, "Hello")
        XCTAssertEqual(secondSnapshot.finishReason, "stop")
    }

    func testExtractCompletedResponseBackfillsOutputFromOutputItemDoneEvent() async throws {
        let runtime = makeRuntime(store: AccountsStore())

        let outputText = try await runtime.withIsolation { runtime in
            let response = try runtime.extractCompletedResponse(
                fromSSE: Data("""
                event: response.output_item.done
                data: {"type":"response.output_item.done","item":{"id":"msg_123","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"OK"}]},"output_index":1,"sequence_number":9}

                event: response.completed
                data: {"type":"response.completed","response":{"id":"resp_123","created_at":1,"model":"gpt-5","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}

                """.utf8)
            )
            let output = response["output"] as? [[String: Any]]
            let content = output?.first?["content"] as? [[String: Any]]
            return content?.first?["text"] as? String
        }

        XCTAssertEqual(outputText, "OK")
    }

    func testPayloadOversizeDetectionFromContentLengthHeader() {
        let oversized = ProxyRuntimeLimits.maxInboundRequestBytes + 1
        let raw = """
        POST /v1/responses HTTP/1.1\r
        Host: 127.0.0.1\r
        Content-Length: \(oversized)\r
        Content-Type: application/json\r
        \r
        {}
        """
        let buffer = Data(raw.utf8)
        XCTAssertTrue(SimpleHTTPServer.isPayloadOversized(buffer: buffer))
    }

    func testPayloadOversizeDetectionFromBufferedBytes() {
        let buffer = Data(repeating: 65, count: ProxyRuntimeLimits.maxInboundRequestBytes + 1)
        XCTAssertTrue(SimpleHTTPServer.isPayloadOversized(buffer: buffer))
    }

    func testPayloadOversizeDoesNotTriggerUnderLimit() {
        let allowed = ProxyRuntimeLimits.maxInboundRequestBytes - 128
        let raw = """
        POST /v1/responses HTTP/1.1\r
        Host: 127.0.0.1\r
        Content-Length: \(allowed)\r
        Content-Type: application/json\r
        \r
        {}
        """
        let buffer = Data(raw.utf8)
        XCTAssertFalse(SimpleHTTPServer.isPayloadOversized(buffer: buffer))
    }

    private func parseJSON(_ data: Data) throws -> [String: Any] {
        let object = try JSONSerialization.jsonObject(with: data)
        return object as? [String: Any] ?? [:]
    }

    private func makeRuntime(
        store: AccountsStore,
        dateProvider: DateProviding = SystemDateProvider()
    ) -> SwiftNativeProxyRuntimeService {
        makeRuntime(
            storeRepository: CountingStoreRepository(store: store),
            authRepository: ExtractingAuthRepository(),
            onAccountsStoreChanged: nil,
            dateProvider: dateProvider
        )
    }

    private func makeRuntime(
        storeRepository: AccountsStoreRepository,
        settingsRepository: SettingsRepository = MockSettingsRepository(),
        authRepository: AuthRepository = ExtractingAuthRepository(),
        onAccountsStoreChanged: (@Sendable () -> Void)? = nil,
        switchAccount: (@Sendable (String) async throws -> Void)? = nil,
        switchModelProvider: (@Sendable (String) async throws -> Void)? = nil,
        dateProvider: DateProviding = SystemDateProvider(),
        environment: [String: String] = [:]
    ) -> SwiftNativeProxyRuntimeService {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let resolvedSwitchAccount = switchAccount ?? { cardID in
            let store = try storeRepository.loadStore()
            guard let account = store.accounts.first(where: { $0.id == cardID }) else {
                throw AppError.invalidData("Missing account for proxy runtime test switch")
            }
            var nextStore = store
            nextStore.currentAccountID = account.id
            nextStore.currentSelection = CurrentAccountSelection(
                    cardID: account.accountID,
                selectedAt: dateProvider.unixMillisecondsNow(),
                sourceDeviceID: "test-runtime"
            )
            try storeRepository.saveStore(nextStore)
            try authRepository.writeCurrentAuth(account.authJSON)
        }
        return SwiftNativeProxyRuntimeService(
            paths: FileSystemPaths(
                applicationSupportDirectory: tempDir,
                accountStorePath: tempDir.appendingPathComponent("accounts.json"),
                settingsStorePath: tempDir.appendingPathComponent("settings.json"),
                codexAuthPath: tempDir.appendingPathComponent("auth.json"),
                codexConfigPath: tempDir.appendingPathComponent("config.toml"),
                proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
                proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key"),
                cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
            ),
            storeRepository: storeRepository,
            settingsRepository: settingsRepository,
            authRepository: authRepository,
            onAccountsStoreChanged: onAccountsStoreChanged,
            switchAccount: resolvedSwitchAccount,
            switchModelProvider: switchModelProvider,
            dateProvider: dateProvider,
            environment: environment
        )
    }

    private func makeStoredAccount(
        id: String,
        label: String,
        accountID: String,
        addedAt: Int64
    ) -> StoredAccount {
        StoredAccount(
            id: id,
            label: label,
            email: nil,
            accountID: accountID,
            planType: nil,
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([
                "tokens": .object([
                    "access_token": .string("token-\(accountID)"),
                    "account_id": .string(accountID)
                ])
            ]),
            addedAt: addedAt,
            updatedAt: addedAt,
            usage: nil,
            usageError: nil
        )
    }
}

private struct FixedDateProvider: DateProviding {
    let unixSeconds: Int64
    let unixMilliseconds: Int64

    func unixSecondsNow() -> Int64 { unixSeconds }
    func unixMillisecondsNow() -> Int64 { unixMilliseconds }
}

private final class InMemoryAccountsStoreRepository: AccountsStoreRepository, @unchecked Sendable {
    var store: AccountsStore

    init(store: AccountsStore) {
        self.store = store
    }

    func loadStore() throws -> AccountsStore {
        store
    }

    func saveStore(_ store: AccountsStore) throws {
        self.store = store
    }
}

private final class MockStoreRepository: AccountsStoreRepository, @unchecked Sendable {
    func loadStore() throws -> AccountsStore {
        AccountsStore()
    }

    func saveStore(_ store: AccountsStore) throws {
    }
}

private final class CountingStoreRepository: AccountsStoreRepository, @unchecked Sendable {
    private let store: AccountsStore
    private(set) var loadStoreCallCount = 0

    init(store: AccountsStore) {
        self.store = store
    }

    func loadStore() throws -> AccountsStore {
        loadStoreCallCount += 1
        return store
    }

    func saveStore(_ store: AccountsStore) throws {
        _ = store
    }
}

private final class MockSettingsRepository: SettingsRepository, @unchecked Sendable {
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

private final class ProviderSwitchRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedProviderIDs: [String] = []

    var providerIDs: [String] {
        lock.withLock { recordedProviderIDs }
    }

    func record(_ providerID: String) {
        lock.withLock {
            recordedProviderIDs.append(providerID)
        }
    }
}

private final class AccountSwitchRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recordedCardIDs: [String] = []

    var cardIDs: [String] {
        lock.withLock { recordedCardIDs }
    }

    func record(_ cardID: String) {
        lock.withLock {
            recordedCardIDs.append(cardID)
        }
    }
}

private final class MockAuthRepository: AuthRepository, @unchecked Sendable {
    func readCurrentAuth() throws -> JSONValue { .null }
    func readCurrentAuthOptional() throws -> JSONValue? { nil }
    func readAuth(from url: URL) throws -> JSONValue {
        _ = url
        return .null
    }
    func writeCurrentAuth(_ auth: JSONValue) throws {}
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue {
        _ = tokens
        return .null
    }
    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        ExtractedAuth(accountID: "acct", accessToken: "token", email: nil, planType: nil, teamName: nil)
    }
}

private final class CountingAuthRepository: AuthRepository, @unchecked Sendable {
    private(set) var extractAuthCallCount = 0

    func readCurrentAuth() throws -> JSONValue { .null }
    func readCurrentAuthOptional() throws -> JSONValue? { nil }
    func readAuth(from url: URL) throws -> JSONValue {
        _ = url
        return .null
    }
    func writeCurrentAuth(_ auth: JSONValue) throws {
        _ = auth
    }
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue {
        _ = tokens
        return .null
    }
    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        _ = auth
        extractAuthCallCount += 1
        return ExtractedAuth(accountID: "acct", accessToken: "token", email: nil, planType: nil, teamName: nil)
    }
}

private final class RecordingAuthRepository: AuthRepository, @unchecked Sendable {
    private(set) var writeCurrentAuthCallCount = 0
    private var currentAuth: JSONValue?

    init(currentAuth: JSONValue? = nil) {
        self.currentAuth = currentAuth
    }

    func readCurrentAuth() throws -> JSONValue { currentAuth ?? .null }
    func readCurrentAuthOptional() throws -> JSONValue? { currentAuth }
    func readAuth(from url: URL) throws -> JSONValue {
        _ = url
        return .null
    }
    func writeCurrentAuth(_ auth: JSONValue) throws {
        currentAuth = auth
        writeCurrentAuthCallCount += 1
    }
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue {
        _ = tokens
        return .null
    }
    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        guard case .object(let root) = auth,
              case .object(let tokens)? = root["tokens"],
              case .string(let accountID)? = tokens["account_id"],
              case .string(let accessToken)? = tokens["access_token"] else {
            throw AppError.invalidData("Missing test auth payload")
        }

        return ExtractedAuth(
            accountID: accountID,
            accessToken: accessToken,
            email: nil,
            planType: nil,
            teamName: nil
        )
    }
}

private final class FailingWriteAuthRepository: AuthRepository, @unchecked Sendable {
    func readCurrentAuth() throws -> JSONValue { .null }
    func readCurrentAuthOptional() throws -> JSONValue? { nil }
    func readAuth(from url: URL) throws -> JSONValue {
        _ = url
        return .null
    }
    func writeCurrentAuth(_ auth: JSONValue) throws {
        _ = auth
        throw AppError.io("write failed")
    }
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue {
        _ = tokens
        return .null
    }
    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        guard case .object(let root) = auth,
              case .object(let tokens)? = root["tokens"],
              case .string(let accountID)? = tokens["account_id"],
              case .string(let accessToken)? = tokens["access_token"] else {
            throw AppError.invalidData("Missing test auth payload")
        }

        return ExtractedAuth(
            accountID: accountID,
            accessToken: accessToken,
            email: nil,
            planType: nil,
            teamName: nil
        )
    }
}

private final class StoreChangeCallback: @unchecked Sendable {
    private var callCount = 0

    func markCalled() {
        callCount += 1
    }

    func readCallCount() -> Int {
        callCount
    }
}

private final class ExtractingAuthRepository: AuthRepository, @unchecked Sendable {
    func readCurrentAuth() throws -> JSONValue { .null }
    func readCurrentAuthOptional() throws -> JSONValue? { nil }
    func readAuth(from url: URL) throws -> JSONValue {
        _ = url
        return .null
    }
    func writeCurrentAuth(_ auth: JSONValue) throws {
        _ = auth
    }
    func removeCurrentAuth() throws {}
    func makeChatGPTAuth(from tokens: ChatGPTOAuthTokens) throws -> JSONValue {
        _ = tokens
        return .null
    }
    func extractAuth(from auth: JSONValue) throws -> ExtractedAuth {
        guard case .object(let root) = auth,
              case .object(let tokens)? = root["tokens"],
              case .string(let accountID)? = tokens["account_id"],
              case .string(let accessToken)? = tokens["access_token"] else {
            throw AppError.invalidData("Missing test auth payload")
        }

        return ExtractedAuth(
            accountID: accountID,
            accessToken: accessToken,
            email: nil,
            planType: nil,
            teamName: nil
        )
    }
}

private struct NormalizedResponsesSnapshot: Sendable {
    let downstreamStream: Bool
    let model: String?
    let stream: Bool?
    let input: [NormalizedInputMessage]
}

private struct StreamedChatChunkSnapshot: Sendable {
    let chunkCount: Int
    let firstContent: String?
    let finishReason: String?
}

private struct NormalizedInputMessage: Sendable {
    let type: String?
    let role: String?
    let content: [NormalizedInputContent]
}

private struct NormalizedInputContent: Sendable {
    let type: String?
    let text: String?
}

private extension SwiftNativeProxyRuntimeService {
    func withIsolation<T: Sendable>(
        _ body: @Sendable (isolated SwiftNativeProxyRuntimeService) throws -> T
    ) async rethrows -> T {
        try body(self)
    }
}

private func snapshot(
    from normalized: (payload: [String: Any], downstreamStream: Bool)
) -> NormalizedResponsesSnapshot {
    let input = (normalized.payload["input"] as? [[String: Any]] ?? []).map { message in
        NormalizedInputMessage(
            type: message["type"] as? String,
            role: message["role"] as? String,
            content: (message["content"] as? [[String: Any]] ?? []).map { item in
                NormalizedInputContent(
                    type: item["type"] as? String,
                    text: item["text"] as? String
                )
            }
        )
    }

    return NormalizedResponsesSnapshot(
        downstreamStream: normalized.downstreamStream,
        model: normalized.payload["model"] as? String,
        stream: normalized.payload["stream"] as? Bool,
        input: input
    )
}
