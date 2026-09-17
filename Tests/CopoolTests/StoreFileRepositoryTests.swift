import XCTest
@testable import Copool

final class StoreFileRepositoryTests: XCTestCase {
    func testSub2APICacheMigratesAndSurvivesStaleAccountStoreSave() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let paths = proxyPersistencePaths(directory)
        let accountRepository = StoreFileRepository(paths: paths)
        let staleStore = try accountRepository.loadStore()
        let cached = Sub2APIAccountSummary(
            id: 42, name: "Cached", email: nil, accountID: nil, accountType: "oauth",
            status: "active", planType: nil, usage: nil, usageError: nil
        )
        let first = Sub2APIProviderConfiguration(providerID: "first", importedAccountIDs: [42], cachedAccounts: [cached])
        let second = Sub2APIProviderConfiguration(providerID: "second", importedAccountIDs: [42], cachedAccounts: [cached])
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(providers: [first, second])
        try JSONEncoder().encode(settings).write(to: paths.settingsStorePath)

        let repository = SettingsFileRepository(paths: paths)
        XCTAssertEqual(try repository.loadSettings(), settings)
        let rawSettings = try String(contentsOf: paths.settingsStorePath, encoding: .utf8)
        XCTAssertFalse(rawSettings.contains("cachedAccounts"))
        XCTAssertEqual(try accountRepository.loadStore().cachedAccounts[first.id.uuidString], [cached])
        XCTAssertEqual(try accountRepository.loadStore().cachedAccounts[second.id.uuidString], [cached])

        try accountRepository.saveStore(staleStore)
        XCTAssertEqual(try SettingsFileRepository(paths: paths).loadSettings(), settings)
        settings.sub2APIProvider.providers[0].cachedAccounts = []
        try repository.saveSettings(settings)
        XCTAssertEqual(try repository.loadSettings(), settings)
        settings.sub2APIProvider.providers.removeFirst()
        try repository.saveSettings(settings)
        XCTAssertNil(try accountRepository.loadStore().cachedAccounts[first.id.uuidString])
        XCTAssertEqual(try accountRepository.loadStore().cachedAccounts[second.id.uuidString], [cached])
    }

    func testAccountProxyReloadsFromDiskIntoCardSnapshotAfterRepositoryRestart() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let paths = proxyPersistencePaths(directory)
        let proxy = "socks5://127.0.0.1:1080"
        let account = StoredAccount(
            id: "a", label: "A", email: nil, accountID: "a", planType: "pro",
            teamName: nil, teamAlias: nil, authJSON: .null, addedAt: 1, updatedAt: 1,
            usage: nil, usageError: nil, proxyURL: proxy
        )
        try StoreFileRepository(paths: paths).saveStore(AccountsStore(accounts: [account]))
        let restartedRepository = StoreFileRepository(paths: paths)
        XCTAssertEqual(try restartedRepository.loadStore().accountSummaries().first?.proxyURL, proxy)
        _ = try restartedRepository.mutateStore { store in
            store.accounts[0].proxyURL = "http://127.0.0.1:8080"
        }
        XCTAssertEqual(
            try StoreFileRepository(paths: paths).loadStore().accountSummaries().first?.proxyURL,
            "http://127.0.0.1:8080"
        )
    }

    func testSub2APIProxyReloadsEvenWhenCachedCardHasNoProxy() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let paths = proxyPersistencePaths(directory)
        let proxy = "http://127.0.0.1:8080"
        let cached = Sub2APIAccountSummary(
            id: 42, name: "A", email: nil, accountID: nil, accountType: "oauth",
            status: "active", planType: nil, usage: nil, usageError: nil
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(providers: [
            Sub2APIProviderConfiguration(
                providerID: "p", accountProxyURLs: ["42": proxy],
                importedAccountIDs: [42], cachedAccounts: [cached]
            )
        ])
        try SettingsFileRepository(paths: paths).saveSettings(settings)
        let restarted = try SettingsFileRepository(paths: paths).loadSettings()
        let provider = try XCTUnwrap(restarted.sub2APIProvider.normalized().providers.first)
        XCTAssertEqual(provider.cachedAccounts.first?.accountSummary.proxyURL, proxy)
        var cleared = provider
        cleared.accountProxyURLs = [:]
        XCTAssertEqual(cleared.normalized().cachedAccounts.first?.accountSummary.proxyURL, "")
    }


    func testLoadStoreTreatsTrailingGarbageAsCorruption() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let raw = "{\"version\":1,\"accounts\":[],\"settings\":{\"launchAtStartup\":false,\"trayUsageDisplayMode\":\"remaining\",\"launchCodexAfterSwitch\":true,\"syncOpencodeOpenaiAuth\":false,\"restartEditorsOnSwitch\":false,\"restartEditorTargets\":[],\"autoStartApiProxy\":false,\"remoteServers\":[],\"locale\":\"zh-CN\"}}\nINVALID".data(using: .utf8)!
        try raw.write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let repository = StoreFileRepository(paths: paths)
        let store = try repository.loadStore()

        XCTAssertEqual(store, AccountsStore())
        let rewritten = try Data(contentsOf: storePath)
        XCTAssertEqual(try JSONDecoder().decode(AccountsStore.self, from: rewritten), AccountsStore())

        let backups = try FileManager.default.contentsOfDirectory(at: tempDir, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.hasPrefix("accounts.corrupt-") }
        XCTAssertEqual(backups.count, 1)
        XCTAssertEqual(try Data(contentsOf: backups[0]), raw)
    }

    func testLoadStoreBacksUpInvalidStoreAndResetsPrimaryStore() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let invalid = "{\"version\":1,\"accounts\":[".data(using: .utf8)!
        try invalid.write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let repository = StoreFileRepository(paths: paths)
        let store = try repository.loadStore()

        XCTAssertEqual(store, AccountsStore())
        let rewritten = try Data(contentsOf: storePath)
        XCTAssertNotEqual(rewritten, invalid)
        XCTAssertEqual(try JSONDecoder().decode(AccountsStore.self, from: rewritten), AccountsStore())

        let backups = try FileManager.default.contentsOfDirectory(at: tempDir, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.hasPrefix("accounts.corrupt-") }
        XCTAssertEqual(backups.count, 1)
        XCTAssertEqual(try Data(contentsOf: backups[0]), invalid)
    }

    func testLoadStoreDecodesLegacyIdentityShapeWithoutPrincipalOrSelectionKey() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let legacyRoot: [String: Any] = [
            "version": 1,
            "accounts": [
                StoredAccount(
                    id: "acct-1",
                    label: "Legacy",
                    email: "legacy@example.com",
                    accountID: "legacy-account",
                    planType: "pro",
                    teamName: nil,
                    teamAlias: nil,
                    authJSON: .object([:]),
                    addedAt: 1,
                    updatedAt: 2,
                    usage: nil,
                    usageError: nil,
                    principalID: nil
                )
            ].map { account in
                try! JSONSerialization.jsonObject(with: try! JSONEncoder().encode(account)) as! [String: Any]
            },
            "currentSelection": [
                "accountId": "legacy-account",
                "selectedAt": 123,
                "sourceDeviceID": "device-a",
                "accountKey": "legacy-account"
            ],
            "settings": try! JSONSerialization.jsonObject(with: JSONEncoder().encode(AppSettings.defaultValue))
        ]
        var root = legacyRoot
        var accounts = try XCTUnwrap(root["accounts"] as? [[String: Any]])
        accounts[0].removeValue(forKey: "principalId")
        root["accounts"] = accounts
        var currentSelection = try XCTUnwrap(root["currentSelection"] as? [String: Any])
        currentSelection.removeValue(forKey: "accountKey")
        root["currentSelection"] = currentSelection
        let raw = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
        try raw.write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let repository = StoreFileRepository(paths: paths)
        let store = try repository.loadStore()
        let summaries = store.accountSummaries()

        XCTAssertEqual(store.accounts.count, 1)
        XCTAssertNil(store.accounts[0].principalID)
        XCTAssertNil(store.currentAccountID)
        XCTAssertEqual(store.currentSelection?.cardID, "legacy-account")
        XCTAssertTrue(summaries.filter(\.isCurrent).isEmpty)
    }

    func testLoadStoreUsesCurrentAccountIDAsSingleCurrentSource() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let raw = try JSONSerialization.data(
            withJSONObject: [
                "version": 1,
                "currentAccountId": "acct-2",
                "accounts": [
                    [
                        "id": "acct-1",
                        "label": "First",
                        "email": "first@example.com",
                        "accountId": "account-1",
                        "planType": "pro",
                        "authJson": [:],
                        "addedAt": 1,
                        "updatedAt": 1,
                        "workspaceStatus": "active",
                        "displayStatus": "list"
                    ],
                    [
                        "id": "acct-2",
                        "label": "Second",
                        "email": "second@example.com",
                        "accountId": "account-2",
                        "planType": "pro",
                        "authJson": [:],
                        "addedAt": 2,
                        "updatedAt": 2,
                        "workspaceStatus": "active",
                        "displayStatus": "list"
                    ]
                ]
            ],
            options: [.prettyPrinted, .sortedKeys]
        )
        try raw.write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let repository = StoreFileRepository(paths: paths)
        let store = try repository.loadStore()
        let summaries = store.accountSummaries()

        XCTAssertEqual(summaries.filter(\.isCurrent).map(\.id), ["acct-2"])
    }

    func testLoadStoreDefaultsMissingWorkspaceStatusToActive() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let account = StoredAccount(
            id: "acct-1",
            label: "Legacy",
            email: "legacy@example.com",
            accountID: "legacy-account",
            planType: "team",
            teamName: "workspace-a",
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 2,
            usage: nil,
            usageError: nil
        )
        var rawAccount = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(account)) as? [String: Any]
        )
        rawAccount.removeValue(forKey: "workspaceStatus")
        let raw = try JSONSerialization.data(
            withJSONObject: [
                "version": 1,
                "accounts": [rawAccount]
            ],
            options: [.prettyPrinted, .sortedKeys]
        )
        try raw.write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let repository = StoreFileRepository(paths: paths)
        let store = try repository.loadStore()

        XCTAssertEqual(store.accounts.count, 1)
        XCTAssertEqual(store.accounts[0].workspaceStatus, .active)
        XCTAssertFalse(
            try FileManager.default.contentsOfDirectory(at: tempDir, includingPropertiesForKeys: nil)
                .contains(where: { $0.lastPathComponent.hasPrefix("accounts.corrupt-") })
        )
    }

    func testStoreRoundTripsWorkspaceDirectoryEntries() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: tempDir.appendingPathComponent("settings.json"),
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )
        let repository = StoreFileRepository(paths: paths)
        let store = AccountsStore(
            version: 1,
            accounts: [],
            workspaceDirectory: [
                WorkspaceDirectoryEntry(
                    workspaceID: "workspace-1",
                    workspaceName: "Workspace One",
                    email: "team@example.com",
                    planType: "team",
                    kind: .workspace,
                    status: .deactivated,
                    visibility: .deleted,
                    lastSeenAt: 123,
                    lastStatusCheckedAt: 456
                )
            ],
            currentSelection: nil
        )

        try repository.saveStore(store)
        let loaded = try repository.loadStore()

        XCTAssertEqual(loaded.workspaceDirectory, store.workspaceDirectory)
    }

    func testLoadSettingsMigratesLegacyMergedStoreIntoSeparateFiles() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let storePath = tempDir.appendingPathComponent("accounts.json")
        let settingsPath = tempDir.appendingPathComponent("settings.json")
        let legacySettings = AppSettings(
            launchAtStartup: true,
            launchCodexAfterSwitch: false,
            autoSmartSwitch: true,
            syncOpencodeOpenaiAuth: true,
            restartEditorsOnSwitch: true,
            restartEditorTargets: [.cursor],
            autoStartApiProxy: true,
            remoteServers: [],
            locale: AppLocale.english.identifier
        )
        let account = StoredAccount(
            id: "acct-1",
            label: "Legacy",
            email: "legacy@example.com",
            accountID: "legacy-account",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 2,
            usage: nil,
            usageError: nil
        )
        let legacyRoot: [String: Any] = [
            "version": 1,
            "accounts": [
                try! JSONSerialization.jsonObject(with: JSONEncoder().encode(account))
            ],
            "currentSelection": [
                "accountId": "legacy-account",
                "selectedAt": 123,
                "sourceDeviceID": "device-a"
            ],
            "settings": try! JSONSerialization.jsonObject(with: JSONEncoder().encode(legacySettings))
        ]
        try JSONSerialization.data(withJSONObject: legacyRoot, options: [.prettyPrinted, .sortedKeys]).write(to: storePath)

        let paths = FileSystemPaths(
            applicationSupportDirectory: tempDir,
            accountStorePath: storePath,
            settingsStorePath: settingsPath,
            codexAuthPath: tempDir.appendingPathComponent("auth.json"),
            codexConfigPath: tempDir.appendingPathComponent("config.toml"),
            proxyDaemonDataDirectory: tempDir.appendingPathComponent("proxyd", isDirectory: true),
            proxyDaemonKeyPath: tempDir.appendingPathComponent("proxyd/api-proxy.key", isDirectory: false),
            cloudflaredLogDirectory: tempDir.appendingPathComponent("cloudflared-logs", isDirectory: true)
        )

        let settingsRepository = SettingsFileRepository(paths: paths)
        let migrated = try settingsRepository.loadSettings()
        let migratedAccounts = try JSONDecoder().decode(AccountsStore.self, from: Data(contentsOf: storePath))
        let storedSettings = try JSONDecoder().decode(AppSettings.self, from: Data(contentsOf: settingsPath))

        XCTAssertEqual(migrated, legacySettings)
        XCTAssertEqual(storedSettings, legacySettings)
        XCTAssertEqual(migratedAccounts.accounts, [account])
        XCTAssertEqual(migratedAccounts.currentSelection?.cardID, "legacy-account")
    }

    func testAccountSummariesMarkOnlyMatchingVariantAsCurrent() {
        let firstAccount = StoredAccount(
            id: "acct-1",
            label: "First",
            email: "first@example.com",
            accountID: "account-1",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 1,
            usage: nil,
            usageError: nil,
            principalID: "principal-1"
        )
        let secondAccount = StoredAccount(
            id: "acct-2",
            label: "Second",
            email: "second@example.com",
            accountID: "account-1",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 1,
            usage: nil,
            usageError: nil,
            principalID: "principal-2"
        )
        let store = AccountsStore(
            version: 1,
            accounts: [firstAccount, secondAccount],
            currentAccountID: secondAccount.id,
            currentSelection: CurrentAccountSelection(
                cardID: secondAccount.id,
                selectedAt: 123,
                sourceDeviceID: "device-a"
            )
        )

        let summaries = store.accountSummaries()

        XCTAssertEqual(summaries.filter(\.isCurrent).count, 1)
        XCTAssertEqual(summaries.first(where: \.isCurrent)?.id, secondAccount.id)
    }

    func testAccountSummariesUseCurrentAccountIDAsSingleCurrentSource() {
        let account = StoredAccount(
            id: "acct-1",
            label: "Remote Selected",
            email: "remote@example.com",
            accountID: "remote-account",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 2,
            usage: nil,
            usageError: nil
        )
        let otherAccount = StoredAccount(
            id: "acct-2",
            label: "Local Auth",
            email: "local@example.com",
            accountID: "local-account",
            planType: "pro",
            teamName: nil,
            teamAlias: nil,
            authJSON: .object([:]),
            addedAt: 1,
            updatedAt: 2,
            usage: nil,
            usageError: nil
        )
        let store = AccountsStore(
            version: 1,
            accounts: [account, otherAccount],
            currentAccountID: account.id,
            currentSelection: CurrentAccountSelection(
                cardID: "remote-account",
                selectedAt: 123,
                sourceDeviceID: "device-a"
            )
        )

        let summaries = store.accountSummaries()

        XCTAssertEqual(
            summaries.first(where: { $0.accountID == "remote-account" })?.isCurrent,
            true
        )
        XCTAssertEqual(
            summaries.first(where: { $0.accountID == "local-account" })?.isCurrent,
            false
        )
    }
}


private func proxyPersistencePaths(_ directory: URL) -> FileSystemPaths {
    FileSystemPaths(
        applicationSupportDirectory: directory,
        accountStorePath: directory.appendingPathComponent("accounts.json"),
        settingsStorePath: directory.appendingPathComponent("settings.json"),
        codexAuthPath: directory.appendingPathComponent("auth.json"),
        codexConfigPath: directory.appendingPathComponent("config.toml"),
        proxyDaemonDataDirectory: directory.appendingPathComponent("proxyd"),
        proxyDaemonKeyPath: directory.appendingPathComponent("proxyd/key"),
        cloudflaredLogDirectory: directory.appendingPathComponent("logs")
    )
}
