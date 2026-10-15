import Foundation
import SwiftUI
import WidgetKit

enum AccountsWidgetConfiguration {
    /// The app group the build signed the widget for, from its Info.plist.
    static let appGroupIdentifier = (Bundle.main.object(forInfoDictionaryKey: "CopoolAppGroup") as? String)
        .flatMap { $0.isEmpty ? nil : $0 }
    static let usageProgressDisplayModeDefaultsKey = "accountsWidgetUsageProgressDisplayMode"
}

/// A widget the app publishes; each has its own kind and snapshot, as the app names them.
enum AccountsWidgetVariant: Sendable {
    case codex
    case claude
    /// Both pools.
    case all

    var kind: String {
        switch self {
        case .codex: "CopoolCodexAccountsWidget"
        case .claude: "CopoolClaudeAccountsWidget"
        case .all: "CopoolAccountsWidget"
        }
    }

    var snapshotFilename: String {
        switch self {
        case .codex: "codex-accounts-widget-snapshot.json"
        case .claude: "claude-accounts-widget-snapshot.json"
        case .all: "accounts-widget-snapshot.json"
        }
    }
}

enum AccountsWidgetUsageProgressDisplayMode: String, Codable, Equatable, Sendable {
    case used
    case remaining
}

struct AccountsWidgetDisplayModeStore: Sendable {
    private let defaultsProvider: @Sendable () -> UserDefaults?

    init(
        defaultsProvider: @escaping @Sendable () -> UserDefaults? = {
            AccountsWidgetConfiguration.appGroupIdentifier.flatMap { UserDefaults(suiteName: $0) }
        }
    ) {
        self.defaultsProvider = defaultsProvider
    }

    func load() -> AccountsWidgetUsageProgressDisplayMode {
        guard let rawValue = defaultsProvider()?
            .string(forKey: AccountsWidgetConfiguration.usageProgressDisplayModeDefaultsKey),
              let mode = AccountsWidgetUsageProgressDisplayMode(rawValue: rawValue) else {
            return .used
        }
        return mode
    }

    func save(rawValue: String) {
        let normalizedRawValue = AccountsWidgetUsageProgressDisplayMode(rawValue: rawValue)?.rawValue
            ?? AccountsWidgetUsageProgressDisplayMode.used.rawValue
        defaultsProvider()?.set(
            normalizedRawValue,
            forKey: AccountsWidgetConfiguration.usageProgressDisplayModeDefaultsKey
        )
    }
}

struct AccountsWidgetSnapshot: Codable, Equatable, Sendable {
    var generatedAt: Int64
    var usageProgressDisplayMode: AccountsWidgetUsageProgressDisplayMode
    var currentCard: AccountsWidgetCardSnapshot?
    var secondaryCard: AccountsWidgetCardSnapshot?
    var rows: [AccountsWidgetRowSnapshot]

    static let empty = AccountsWidgetSnapshot(
        generatedAt: 0,
        usageProgressDisplayMode: .used,
        currentCard: nil,
        secondaryCard: nil,
        rows: []
    )

    func resolvedUsageProgressDisplayMode() -> AccountsWidgetUsageProgressDisplayMode {
        return usageProgressDisplayMode
    }
}

struct AccountsWidgetResolvedColor: Equatable, Sendable {
    let red: Double
    let green: Double
    let blue: Double
    let opacity: Double

    var color: Color {
        Color(red: red, green: green, blue: blue, opacity: opacity)
    }
}

struct AccountsWidgetTagPalette: Equatable, Sendable {
    let fill: AccountsWidgetResolvedColor
    let text: AccountsWidgetResolvedColor

    static let accentedContrast = AccountsWidgetTagPalette(
        fill: AccountsWidgetResolvedColor(red: 1, green: 1, blue: 1, opacity: 0.18),
        text: AccountsWidgetResolvedColor(red: 1, green: 1, blue: 1, opacity: 0.98)
    )
}

enum AccountsWidgetTagPaletteResolver {
    static func planTagPalette(
        for planLabel: String,
        colorScheme: ColorScheme,
        renderingMode: WidgetRenderingMode
    ) -> AccountsWidgetTagPalette {
        guard renderingMode == .fullColor else {
            return .accentedContrast
        }

        switch (planLabel, colorScheme) {
        case ("PRO", .dark):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.46, green: 0.30, blue: 0.15, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 1.00, green: 0.73, blue: 0.28, opacity: 1)
            )
        case ("PRO", _):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.98, green: 0.82, blue: 0.63, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.78, green: 0.42, blue: 0.03, opacity: 1)
            )
        case ("PLUS", .dark):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.39, green: 0.19, blue: 0.31, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.98, green: 0.58, blue: 0.82, opacity: 1)
            )
        case ("PLUS", _):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.97, green: 0.78, blue: 0.88, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.70, green: 0.19, blue: 0.44, opacity: 1)
            )
        case ("FREE", .dark):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.24, green: 0.28, blue: 0.37, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.84, green: 0.88, blue: 0.95, opacity: 1)
            )
        case ("FREE", _):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.87, green: 0.89, blue: 0.94, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.34, green: 0.39, blue: 0.50, opacity: 1)
            )
        case (let plan, .dark) where plan.hasPrefix("MAX"):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.42, green: 0.24, blue: 0.19, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 1.00, green: 0.64, blue: 0.50, opacity: 1)
            )
        case (let plan, _) where plan.hasPrefix("MAX"):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.97, green: 0.84, blue: 0.78, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.76, green: 0.36, blue: 0.22, opacity: 1)
            )
        case ("ENTERPRISE", .dark), ("BUSINESS", .dark):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.22, green: 0.27, blue: 0.46, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.70, green: 0.77, blue: 1.00, opacity: 1)
            )
        case ("ENTERPRISE", _), ("BUSINESS", _):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.81, green: 0.85, blue: 0.98, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.24, green: 0.35, blue: 0.69, opacity: 1)
            )
        case (_, .dark):
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.16, green: 0.35, blue: 0.42, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.19, green: 0.86, blue: 0.93, opacity: 1)
            )
        default:
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.71, green: 0.90, blue: 0.94, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.00, green: 0.55, blue: 0.65, opacity: 1)
            )
        }
    }

    /// The pool tag on the widget showing both pools, neutral beside the plan's color.
    static func poolTagPalette(
        for colorScheme: ColorScheme,
        renderingMode: WidgetRenderingMode
    ) -> AccountsWidgetTagPalette {
        guard renderingMode == .fullColor else {
            return .accentedContrast
        }

        switch colorScheme {
        case .dark:
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 1.00, green: 1.00, blue: 1.00, opacity: 0.14),
                text: AccountsWidgetResolvedColor(red: 1.00, green: 1.00, blue: 1.00, opacity: 0.78)
            )
        default:
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.12, green: 0.16, blue: 0.22, opacity: 0.10),
                text: AccountsWidgetResolvedColor(red: 0.33, green: 0.39, blue: 0.48, opacity: 1)
            )
        }
    }

    static func accountTagPalette(
        for colorScheme: ColorScheme,
        renderingMode: WidgetRenderingMode
    ) -> AccountsWidgetTagPalette {
        guard renderingMode == .fullColor else {
            return .accentedContrast
        }

        switch colorScheme {
        case .dark:
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.37, green: 0.29, blue: 0.28, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 1.00, green: 0.63, blue: 0.19, opacity: 1)
            )
        default:
            return AccountsWidgetTagPalette(
                fill: AccountsWidgetResolvedColor(red: 0.98, green: 0.79, blue: 0.73, opacity: 1),
                text: AccountsWidgetResolvedColor(red: 0.91, green: 0.45, blue: 0.06, opacity: 1)
            )
        }
    }
}

struct AccountsWidgetCardSnapshot: Codable, Equatable, Sendable, Identifiable {
    var id: String
    /// The card's pool, on the widget showing both.
    var poolLabel: String?
    var planLabel: String
    var workspaceLabel: String?
    var accountLabel: String
    var fiveHour: AccountsWidgetWindowSnapshot
    var oneWeek: AccountsWidgetWindowSnapshot
}

struct AccountsWidgetWindowSnapshot: Codable, Equatable, Sendable {
    var title: String
    var progressFraction: Double
    var usedText: String
    var remainingText: String
    var resetText: String
}

struct AccountsWidgetRowSnapshot: Codable, Equatable, Sendable, Identifiable {
    var id: String
    var poolLabel: String?
    var planLabel: String
    var workspaceLabel: String?
    var accountLabel: String
    var fiveHour: AccountsWidgetWindowSnapshot
    var oneWeek: AccountsWidgetWindowSnapshot
}
