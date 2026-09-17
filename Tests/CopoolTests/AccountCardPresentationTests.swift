import XCTest
@testable import Copool

final class AccountCardPresentationTests: XCTestCase {
    func testCurrentAccountPaletteUsesOwnAccentForSelectionHighlight() {
        let palette = AccountCardPalette(accent: .orange, isCurrent: true)

        XCTAssertEqual(palette.selectionBorderAccent, .orange)
        XCTAssertNotNil(palette.surfaceTint)
    }

    func testCollapsedPresentationUsesAliasAccentAndShortEmail() {
        let account = AccountSummary(
            id: "acct-1",
            label: "Primary",
            email: "dev@example.com",
            accountID: "account-1",
            planType: "business",
            teamName: "workspace-a",
            teamAlias: "Alias A",
            addedAt: 1,
            updatedAt: 2,
            usage: UsageSnapshot(
                fetchedAt: 3,
                planType: "business",
                fiveHour: UsageWindow(usedPercent: 27.2, windowSeconds: 18_000, resetAt: 1_763_216_000),
                oneWeek: UsageWindow(usedPercent: 52.6, windowSeconds: 604_800, resetAt: 1_763_820_800),
                credits: CreditSnapshot(hasCredits: true, unlimited: false, balance: "128")
            ),
            usageError: nil,
            isCurrent: true
        )

        let presentation = AccountCardPresentation(
            account: account,
            isCollapsed: true,
            locale: Locale(identifier: "en_US_POSIX"),
            usageProgressDisplayMode: .used
        )

        XCTAssertEqual(presentation.accent, .indigo)
        XCTAssertEqual(presentation.planLabel, "BUSINESS")
        XCTAssertEqual(presentation.teamNameTag, "Alias A")
        XCTAssertEqual(presentation.displayAccountName, "dev")
        XCTAssertEqual(presentation.creditsText, "128")
        XCTAssertEqual(presentation.compactUsage.fiveHourDisplayPercent, 27.2)
        XCTAssertEqual(presentation.compactUsage.oneWeekDisplayPercent, 52.6)
    }

    func testExpandedPresentationFallsBackToTeamAccentAndMissingWindowDefaults() {
        let account = AccountSummary(
            id: "acct-2",
            label: "Backup",
            email: nil,
            accountID: "account-2",
            planType: nil,
            teamName: nil,
            teamAlias: nil,
            addedAt: 1,
            updatedAt: 2,
            usage: UsageSnapshot(
                fetchedAt: 3,
                planType: nil,
                fiveHour: nil,
                oneWeek: nil,
                credits: CreditSnapshot(hasCredits: false, unlimited: true, balance: nil)
            ),
            usageError: nil,
            isCurrent: false
        )

        let presentation = AccountCardPresentation(
            account: account,
            isCollapsed: false,
            locale: Locale(identifier: "en_US"),
            usageProgressDisplayMode: .used
        )

        XCTAssertEqual(presentation.accent, .teal)
        XCTAssertEqual(presentation.planLabel, "TEAM")
        XCTAssertNil(presentation.teamNameTag)
        XCTAssertEqual(presentation.displayAccountName, "account-2")
        XCTAssertEqual(presentation.creditsText, L10n.tr("accounts.card.unlimited"))
        XCTAssertEqual(presentation.fiveHourWindow.progressPercent, 100)
        XCTAssertEqual(presentation.fiveHourWindow.resetText, L10n.tr("accounts.window.reset_at_format", "--"))
    }

    func testPresentationShowsDeactivatedStatusTag() {
        let account = AccountSummary(
            id: "acct-3",
            label: "Workspace",
            email: "dev@example.com",
            accountID: "account-3",
            planType: "business",
            teamName: "workspace-a",
            teamAlias: nil,
            addedAt: 1,
            updatedAt: 2,
            usage: nil,
            usageError: nil,
            workspaceStatus: .deactivated,
            isCurrent: false
        )

        let presentation = AccountCardPresentation(
            account: account,
            isCollapsed: false,
            locale: Locale(identifier: "en_US_POSIX"),
            usageProgressDisplayMode: .used
        )

        XCTAssertEqual(presentation.statusLabel, L10n.tr("accounts.card.status.deactivated"))
    }

    func testRemainingModeUsesRemainingForProgressAndCompactPercent() {
        let account = AccountSummary(
            id: "acct-4",
            label: "Primary",
            email: "dev@example.com",
            accountID: "account-4",
            planType: "team",
            teamName: "workspace-a",
            teamAlias: nil,
            addedAt: 1,
            updatedAt: 2,
            usage: UsageSnapshot(
                fetchedAt: 3,
                planType: "team",
                fiveHour: UsageWindow(usedPercent: 72, windowSeconds: 18_000, resetAt: 1_763_216_000),
                oneWeek: UsageWindow(usedPercent: 30, windowSeconds: 604_800, resetAt: 1_763_820_800),
                credits: nil
            ),
            usageError: nil,
            isCurrent: false
        )

        let presentation = AccountCardPresentation(
            account: account,
            isCollapsed: false,
            locale: Locale(identifier: "en_US"),
            usageProgressDisplayMode: .remaining
        )

        XCTAssertEqual(presentation.fiveHourWindow.progressPercent, 28)
        XCTAssertEqual(presentation.fiveHourWindow.primaryText, L10n.tr("accounts.window.remaining_format", "28%"))
        XCTAssertEqual(presentation.fiveHourWindow.secondaryText, L10n.tr("accounts.window.used_format", "72%"))
        XCTAssertEqual(
            presentation.fiveHourWindow.resetText,
            L10n.tr("accounts.window.reset_at_format", "11/15/25, 22:13:20")
        )
        XCTAssertEqual(presentation.compactUsage.fiveHourDisplayPercent, 28)
        XCTAssertEqual(presentation.compactUsage.oneWeekDisplayPercent, 70)
    }
}
