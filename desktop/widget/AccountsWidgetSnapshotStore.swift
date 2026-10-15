import Foundation
import os

struct AccountsWidgetSnapshotStore: @unchecked Sendable {
    private let logger = Logger(subsystem: "Copool", category: "AccountsWidgetSnapshotStore")
    private let fileManager: FileManager
    private let snapshotURLProvider: @Sendable () -> URL?

    init(
        fileManager: FileManager = .default,
        snapshotURLProvider: @escaping @Sendable () -> URL?
    ) {
        self.fileManager = fileManager
        self.snapshotURLProvider = snapshotURLProvider
    }

    init(variant: AccountsWidgetVariant) {
        let filename = variant.snapshotFilename
        self.init {
            AccountsWidgetConfiguration.appGroupIdentifier
                .flatMap {
                    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0)
                }?
                .appendingPathComponent(filename, isDirectory: false)
        }
    }

    func load() -> AccountsWidgetSnapshot {
        guard let url = snapshotURLProvider() else {
            logger.error("Widget snapshot load failed: app group container unavailable.")
            return .empty
        }

        do {
            let data = try Data(contentsOf: url)
            return try JSONDecoder().decode(AccountsWidgetSnapshot.self, from: data)
        } catch {
            logger.error("Widget snapshot load failed at \(url.path, privacy: .public): \(error.localizedDescription, privacy: .public)")
            return .empty
        }
    }

    func save(_ snapshot: AccountsWidgetSnapshot) throws {
        guard let url = snapshotURLProvider() else {
            throw AppError.io("Widget snapshot app group container is unavailable.")
        }

        let parent = url.deletingLastPathComponent()
        try fileManager.createDirectory(at: parent, withIntermediateDirectories: true)

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(snapshot)
        try data.write(to: url, options: .atomic)
    }
}
