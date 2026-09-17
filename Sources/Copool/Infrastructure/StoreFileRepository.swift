import Foundation
#if canImport(Security)
import Security
#endif

// Both repositories update accounts.json; serialize read/modify/write across instances.
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

        do {
            return try decodeStore(from: data)
        } catch {
            try backupCorruptedStore(raw: data)
            let emptyStore = AccountsStore()
            try saveStoreUnlocked(emptyStore)
            return emptyStore
        }
    }

    private func saveStoreUnlocked(_ store: AccountsStore) throws {
        try fileManager.createDirectory(at: paths.applicationSupportDirectory, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]

        let data: Data
        do {
            data = try encoder.encode(store)
        } catch {
            throw AppError.invalidData(L10n.tr("error.store.serialize_failed_format", error.localizedDescription))
        }

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
            return settings
        }

        if fileManager.fileExists(atPath: paths.accountStorePath.path),
           let legacyStore = try decodeLegacyStore(from: paths.accountStorePath) {
            let migratedSettings = legacyStore.settings
            try saveSettings(migratedSettings)
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

        let data: Data
        do {
            data = try encoder.encode(settings)
        } catch {
            throw AppError.invalidData(L10n.tr("error.store.serialize_failed_format", error.localizedDescription))
        }

        let repository = StoreFileRepository(paths: paths, fileManager: fileManager)
        _ = try repository.mutateStore { store in
            store.cachedAccounts = settings.sub2APIProvider.normalized().providers.reduce(into: [:]) { cache, provider in
                cache[provider.id.uuidString] = provider.cachedAccounts
            }
        }
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
