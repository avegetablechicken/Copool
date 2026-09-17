import Foundation
#if canImport(Security)
import Security
#endif

// Both repositories update the account and settings files; serialize across instances.
private let accountFilesLock = NSRecursiveLock()

final class StoreFileRepository: AccountsStoreRepository, @unchecked Sendable {
    private let paths: FileSystemPaths
    private let fileManager: FileManager
    private let dateProvider: DateProviding
    private let lock = accountFilesLock

    init(paths: FileSystemPaths, fileManager: FileManager = .default, dateProvider: DateProviding = SystemDateProvider()) {
        self.paths = paths
        self.fileManager = fileManager
        self.dateProvider = dateProvider
    }

    func loadStore() throws -> AccountsStore {
        lock.lock()
        defer { lock.unlock() }
        return try loadStoreUnlocked()
    }

    func saveStore(_ store: AccountsStore) throws {
        lock.lock()
        defer { lock.unlock() }
        var updated = store
        if fileManager.fileExists(atPath: paths.accountStorePath.path) {
            updated.cachedAccounts = try loadStoreUnlocked().cachedAccounts
        }
        try saveStoreUnlocked(updated)
    }

    func mutateStore(_ transform: (inout AccountsStore) throws -> Void) throws -> AccountsStore {
        lock.lock()
        defer { lock.unlock() }
        var store = try loadStoreUnlocked()
        try transform(&store)
        try saveStoreUnlocked(store)
        return store
    }

    private func loadStoreUnlocked() throws -> AccountsStore {
        let path = paths.accountStorePath
        guard fileManager.fileExists(atPath: path.path) else {
            return AccountsStore()
        }

        let data: Data
        do {
            data = try Data(contentsOf: path)
        } catch {
            throw AppError.io(L10n.tr("error.store.read_failed_format", error.localizedDescription))
        }

        var store: AccountsStore
        do {
            store = try decodeStore(from: data)
        } catch {
            try backupCorruptedStore(raw: data)
            let emptyStore = AccountsStore()
            try saveStoreUnlocked(emptyStore)
            return emptyStore
        }
        // Settings are authoritative, including an explicitly cleared proxy.
        // Keep migration IO outside the corruption handler so settings failures
        // can never cause a valid account store to be reset.
        let proxies = try AccountProxySettings.read(paths: paths)
        for index in store.accounts.indices {
            if let proxy = proxies[store.accounts[index].id] {
                store.accounts[index].proxyURL = proxy
            }
        }
        let root = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let accounts = root?["accounts"] as? [[String: Any]] ?? []
        let cached = root?["cachedAccounts"] as? [String: [[String: Any]]] ?? [:]
        if accounts.contains(where: { $0["proxyURL"] != nil })
            || cached.values.joined().contains(where: { $0["proxyURL"] != nil }) {
            try saveStoreUnlocked(store)
        }
        return store
    }

    private func saveStoreUnlocked(_ store: AccountsStore) throws {
        try fileManager.createDirectory(at: paths.applicationSupportDirectory, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]

        let data: Data
        do {
            var root = try JSONSerialization.jsonObject(with: encoder.encode(store)) as! [String: Any]
            func removingProxy(_ account: [String: Any]) -> [String: Any] {
                var account = account
                account.removeValue(forKey: "proxyURL")
                return account
            }
            root["accounts"] = (root["accounts"] as? [[String: Any]] ?? []).map(removingProxy)
            let cached = root["cachedAccounts"] as? [String: [[String: Any]]] ?? [:]
            root["cachedAccounts"] = cached.mapValues { $0.map(removingProxy) }
            data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
        } catch {
            throw AppError.invalidData(L10n.tr("error.store.serialize_failed_format", error.localizedDescription))
        }

        // Persist settings before dropping the legacy proxy fields from accounts.
        let proxies = store.accounts.reduce(into: [String: String]()) { $0[$1.id] = $1.proxyURL }
        try AccountProxySettings.write(proxies, paths: paths, fileManager: fileManager)
        try writeAtomically(data: data, to: paths.accountStorePath)
    }

    private func decodeStore(from data: Data) throws -> AccountsStore {
        let decoder = JSONDecoder()
        do {
            return try decoder.decode(AccountsStore.self, from: data)
        } catch {
            throw AppError.invalidData(L10n.tr("error.store.invalid_format_format", error.localizedDescription))
        }
    }

    private func backupCorruptedStore(raw: Data) throws {
        let filename = "accounts.corrupt-\(dateProvider.unixSecondsNow()).json"
        let backupPath = paths.applicationSupportDirectory.appendingPathComponent(filename, isDirectory: false)

        try fileManager.createDirectory(at: paths.applicationSupportDirectory, withIntermediateDirectories: true)
        try raw.write(to: backupPath, options: .atomic)
        Self.setPrivatePermissions(at: backupPath)
    }

    private func writeAtomically(data: Data, to destination: URL) throws {
        let tempURL = destination.deletingLastPathComponent()
            .appendingPathComponent(".\(destination.lastPathComponent).tmp-\(UUID().uuidString)", isDirectory: false)

        do {
            try data.write(to: tempURL, options: .withoutOverwriting)
            Self.setPrivatePermissions(at: tempURL)
            _ = try fileManager.replaceItemAt(destination, withItemAt: tempURL)
            Self.setPrivatePermissions(at: destination)
        } catch {
            try? fileManager.removeItem(at: tempURL)
            if !fileManager.fileExists(atPath: destination.path) {
                do {
                    try data.write(to: destination, options: .atomic)
                    Self.setPrivatePermissions(at: destination)
                    return
                } catch {
                    throw AppError.io(L10n.tr("error.store.write_failed_format", error.localizedDescription))
                }
            }
            throw AppError.io(L10n.tr("error.store.atomic_write_failed_format", error.localizedDescription))
        }
    }

    private static func setPrivatePermissions(at url: URL) {
        #if canImport(Darwin)
        _ = chmod(url.path, S_IRUSR | S_IWUSR)
        #endif
    }
}

enum AccountProxySettings {
    static func read(paths: FileSystemPaths) throws -> [String: String] {
        try read(from: paths.settingsStorePath)
    }

    static func read(from settingsPath: URL) throws -> [String: String] {
        guard FileManager.default.fileExists(atPath: settingsPath.path) else { return [:] }
        let data = try Data(contentsOf: settingsPath)
        let root = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let value = root?["accountProxyURLs"] else { return [:] }
        guard let proxies = value as? [String: String] else {
            throw AppError.invalidData("Invalid accountProxyURLs in settings.json")
        }
        return proxies
    }

    static func write(_ proxies: [String: String], paths: FileSystemPaths, fileManager: FileManager) throws {
        var root: [String: Any]
        if fileManager.fileExists(atPath: paths.settingsStorePath.path) {
            let data = try Data(contentsOf: paths.settingsStorePath)
            guard let existing = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw AppError.invalidData("Invalid settings.json")
            }
            root = existing
        } else {
            // Preserve settings from the oldest combined accounts/settings format.
            let legacy = try? Data(contentsOf: paths.accountStorePath)
            let legacyRoot = legacy.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            if let settings = legacyRoot?["settings"] as? [String: Any] {
                root = settings
            } else {
                root = try JSONSerialization.jsonObject(with: JSONEncoder().encode(AppSettings.defaultValue)) as! [String: Any]
            }
        }
        if root["accountProxyURLs"] as? [String: String] == proxies { return }
        root["accountProxyURLs"] = proxies
        let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys])
        try SettingsFileRepository(paths: paths, fileManager: fileManager)
            .writeAtomically(data: data, to: paths.settingsStorePath)
    }
}

final class SettingsFileRepository: SettingsRepository, @unchecked Sendable {
    private struct LegacyAccountsStore: Codable {
        var version: Int = 1
        var accounts: [StoredAccount] = []
        var currentSelection: CurrentAccountSelection?
        var settings: AppSettings = .defaultValue
    }

    private let paths: FileSystemPaths
    private let fileManager: FileManager

    init(paths: FileSystemPaths, fileManager: FileManager = .default) {
        self.paths = paths
        self.fileManager = fileManager
    }

    func loadSettings() throws -> AppSettings {
        accountFilesLock.lock()
        defer { accountFilesLock.unlock() }
        if fileManager.fileExists(atPath: paths.settingsStorePath.path) {
            var settings = try decodeSettings(from: paths.settingsStorePath)
            let repository = StoreFileRepository(paths: paths, fileManager: fileManager)
            let cache = try repository.loadStore().cachedAccounts
            for index in settings.sub2APIProvider.providers.indices {
                let id = settings.sub2APIProvider.providers[index].id.uuidString
                if let accounts = cache[id] {
                    settings.sub2APIProvider.providers[index].cachedAccounts = accounts
                }
            }
            let raw = try Data(contentsOf: paths.settingsStorePath)
            let root = try JSONSerialization.jsonObject(with: raw) as? [String: Any]
            let provider = root?["sub2APIProvider"] as? [String: Any]
            let configurations = provider?["providers"] as? [[String: Any]] ?? []
            if provider?["cachedAccounts"] != nil || configurations.contains(where: { $0["cachedAccounts"] != nil }) {
                // Save the accounts first; only then remove the legacy cache from settings.
                try saveSettings(settings)
            }
            settings.accountProxyURLs = try AccountProxySettings.read(paths: paths)
            settings.sub2APIProvider = settings.sub2APIProvider.normalized()
            return settings
        }

        if fileManager.fileExists(atPath: paths.accountStorePath.path),
           let legacyStore = try decodeLegacyStore(from: paths.accountStorePath) {
            var migratedSettings = legacyStore.settings
            try saveSettings(migratedSettings)
            migratedSettings.accountProxyURLs = try AccountProxySettings.read(paths: paths)
            return migratedSettings
        }

        return .defaultValue
    }

    func saveSettings(_ settings: AppSettings) throws {
        accountFilesLock.lock()
        defer { accountFilesLock.unlock() }
        try fileManager.createDirectory(at: paths.applicationSupportDirectory, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.userInfo[.omitSub2APIAccountCache] = true

        let repository = StoreFileRepository(paths: paths, fileManager: fileManager)
        _ = try repository.mutateStore { store in
            store.cachedAccounts = settings.sub2APIProvider.normalized().providers.reduce(into: [:]) { cache, provider in
                cache[provider.id.uuidString] = provider.cachedAccounts
            }
        }
        var persistedSettings = settings
        persistedSettings.accountProxyURLs = try AccountProxySettings.read(paths: paths)
        let data = try encoder.encode(persistedSettings)
        try writeAtomically(data: data, to: paths.settingsStorePath)
    }

    private func decodeSettings(from path: URL) throws -> AppSettings {
        let data: Data
        do {
            data = try Data(contentsOf: path)
        } catch {
            throw AppError.io(L10n.tr("error.store.read_failed_format", error.localizedDescription))
        }

        do {
            return try JSONDecoder().decode(AppSettings.self, from: data)
        } catch {
            throw AppError.invalidData(L10n.tr("error.store.invalid_format_format", error.localizedDescription))
        }
    }

    private func decodeLegacyStore(from path: URL) throws -> LegacyAccountsStore? {
        let data = try Data(contentsOf: path)
        return try? JSONDecoder().decode(LegacyAccountsStore.self, from: data)
    }

    fileprivate func writeAtomically(data: Data, to destination: URL) throws {
        let tempURL = destination.deletingLastPathComponent()
            .appendingPathComponent(".\(destination.lastPathComponent).tmp-\(UUID().uuidString)", isDirectory: false)

        do {
            try data.write(to: tempURL, options: .withoutOverwriting)
            Self.setPrivatePermissions(at: tempURL)
            _ = try fileManager.replaceItemAt(destination, withItemAt: tempURL)
            Self.setPrivatePermissions(at: destination)
        } catch {
            try? fileManager.removeItem(at: tempURL)
            if !fileManager.fileExists(atPath: destination.path) {
                do {
                    try data.write(to: destination, options: .atomic)
                    Self.setPrivatePermissions(at: destination)
                    return
                } catch {
                    throw AppError.io(L10n.tr("error.store.write_failed_format", error.localizedDescription))
                }
            }
            throw AppError.io(L10n.tr("error.store.atomic_write_failed_format", error.localizedDescription))
        }
    }

    private static func setPrivatePermissions(at url: URL) {
        #if canImport(Darwin)
        _ = chmod(url.path, S_IRUSR | S_IWUSR)
        #endif
    }
}

final class Sub2APIKeychainSecretStore: Sub2APISecretStoreProtocol, @unchecked Sendable {
    private let service = "com.alick.copool.sub2api"

    func password(for configurationID: UUID) throws -> String? {
        #if canImport(Security)
        var query = baseQuery(for: configurationID)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw keychainError(status)
        }
        guard let password = String(data: data, encoding: .utf8) else {
            throw AppError.invalidData("Keychain returned invalid Sub2api password data.")
        }
        return password
        #else
        _ = configurationID
        throw AppError.io("Keychain is unavailable on this platform.")
        #endif
    }

    func setPassword(_ password: String, for configurationID: UUID) throws {
        #if canImport(Security)
        let data = Data(password.utf8)
        let query = baseQuery(for: configurationID)
        let updateStatus = SecItemUpdate(
            query as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw keychainError(updateStatus)
        }

        var attributes = query
        attributes[kSecValueData as String] = data
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(attributes as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw keychainError(addStatus)
        }
        #else
        _ = password
        _ = configurationID
        throw AppError.io("Keychain is unavailable on this platform.")
        #endif
    }

    func removePassword(for configurationID: UUID) throws {
        #if canImport(Security)
        let status = SecItemDelete(baseQuery(for: configurationID) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw keychainError(status)
        }
        #else
        _ = configurationID
        throw AppError.io("Keychain is unavailable on this platform.")
        #endif
    }

    #if canImport(Security)
    private func baseQuery(for configurationID: UUID) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: configurationID.uuidString,
        ]
    }

    private func keychainError(_ status: OSStatus) -> AppError {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
        return AppError.io("Keychain operation failed: \(message)")
    }
    #endif
}
