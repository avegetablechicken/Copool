import Foundation

enum AccountCardAccent: Equatable {
    case orange
    case pink
    case gray
    case indigo
    case teal
}

struct AccountWindowPresentation: Equatable {
    let title: String
    let progressPercent: Double
    let primaryText: String
    let secondaryText: String
    let resetText: String
}

struct AccountCompactUsagePresentation: Equatable {
    let fiveHourDisplayPercent: Double?
    let oneWeekDisplayPercent: Double?
}

struct AccountCardPresentation: Equatable {
    let accent: AccountCardAccent
    let planLabel: String
    let teamNameTag: String?
    let statusLabel: String?
    let displayAccountName: String
    let creditsText: String
    let fiveHourWindow: AccountWindowPresentation
    let oneWeekWindow: AccountWindowPresentation
    let compactUsage: AccountCompactUsagePresentation

    init(
        account: AccountSummary,
        isCollapsed: Bool,
        locale: Locale,
        usageProgressDisplayMode: UsageProgressDisplayMode
    ) {
        let planLabel = account.normalizedPlanLabel
        self.planLabel = planLabel
        accent = Self.accent(for: planLabel)
        teamNameTag = account.sourceTag
            ?? (account.shouldDisplayWorkspaceTag ? account.displayTeamName : nil)
        statusLabel = account.isWorkspaceDeactivated ? L10n.tr("accounts.card.status.deactivated") : nil
        displayAccountName = Self.displayName(for: account, isCollapsed: isCollapsed)
        creditsText = Self.creditsText(for: account)
        fiveHourWindow = Self.windowPresentation(
            title: L10n.tr("accounts.window.five_hour"),
            window: account.usage?.fiveHour,
            locale: locale,
            usageProgressDisplayMode: usageProgressDisplayMode
        )
        oneWeekWindow = Self.windowPresentation(
            title: L10n.tr("accounts.window.one_week"),
            window: account.usage?.oneWeek,
            locale: locale,
            usageProgressDisplayMode: usageProgressDisplayMode
        )
        compactUsage = AccountCompactUsagePresentation(
            fiveHourDisplayPercent: Self.compactDisplayPercent(
                account.usage?.fiveHour,
                usageProgressDisplayMode: usageProgressDisplayMode
            ),
            oneWeekDisplayPercent: Self.compactDisplayPercent(
                account.usage?.oneWeek,
                usageProgressDisplayMode: usageProgressDisplayMode
            )
        )
    }

    private static func accent(for planLabel: String) -> AccountCardAccent {
        switch planLabel {
        case "PRO":
            .orange
        case "PLUS":
            .pink
        case "FREE":
            .gray
        case "ENTERPRISE", "BUSINESS":
            .indigo
        default:
            .teal
        }
    }

    private static func displayName(for account: AccountSummary, isCollapsed: Bool) -> String {
        AccountDisplayNameFormatter.format(
            account: account,
            style: isCollapsed ? .localPart : .full
        )
    }

    private static func creditsText(for account: AccountSummary) -> String {
        guard let credits = account.usage?.credits else { return "--" }
        if credits.unlimited { return L10n.tr("accounts.card.unlimited") }
        return credits.balance ?? "--"
    }

    private static func windowPresentation(
        title: String,
        window: UsageWindow?,
        locale: Locale,
        usageProgressDisplayMode: UsageProgressDisplayMode
    ) -> AccountWindowPresentation {
        let usedRaw = clamped(window?.usedPercent, fallback: 100)
        let used = roundedPercent(usedRaw)
        let remaining = max(0, 100 - used)
        let progress = usageProgressDisplayMode == .remaining ? remaining : used
        let primaryText = usageProgressDisplayMode == .remaining
            ? L10n.tr("accounts.window.remaining_format", percent(remaining))
            : L10n.tr("accounts.window.used_format", percent(used))
        let secondaryText = usageProgressDisplayMode == .remaining
            ? L10n.tr("accounts.window.used_format", percent(used))
            : L10n.tr("accounts.window.remaining_format", percent(remaining))
        return AccountWindowPresentation(
            title: title,
            progressPercent: progress,
            primaryText: primaryText,
            secondaryText: secondaryText,
            resetText: L10n.tr("accounts.window.reset_at_format", formatResetAt(window?.resetAt, locale: locale))
        )
    }

    private static func compactDisplayPercent(
        _ window: UsageWindow?,
        usageProgressDisplayMode: UsageProgressDisplayMode
    ) -> Double? {
        guard let used = window?.usedPercent else { return nil }
        let clampedUsed = clamped(used, fallback: used)
        return usageProgressDisplayMode == .remaining ? max(0, 100 - clampedUsed) : clampedUsed
    }

    private static func clamped(_ value: Double?, fallback: Double) -> Double {
        guard let value else { return fallback }
        return max(0, min(100, value))
    }

    private static func roundedPercent(_ value: Double) -> Double {
        Double(Int(value.rounded()))
    }

    private static func percent(_ value: Double) -> String {
        "\(Int(value.rounded()))%"
    }

    private static func formatResetAt(_ epoch: Int64?, locale: Locale) -> String {
        guard let epoch else { return "--" }
        let date = Date(timeIntervalSince1970: TimeInterval(epoch))

        let dateFormatter = DateFormatter()
        dateFormatter.locale = locale
        dateFormatter.dateStyle = .short
        dateFormatter.timeStyle = .none

        let timeFormatter = DateFormatter()
        timeFormatter.locale = locale
        timeFormatter.setLocalizedDateFormatFromTemplate("HHmmss")

        return "\(dateFormatter.string(from: date)), \(timeFormatter.string(from: date))"
    }
}
