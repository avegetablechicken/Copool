import SwiftUI
import WidgetKit

@main
struct CopoolWidgets: WidgetBundle {
    var body: some Widget {
        CopoolAccountsWidget()
        CopoolCodexAccountsWidget()
        CopoolClaudeAccountsWidget()
    }
}

struct CopoolAccountsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(
            kind: AccountsWidgetVariant.all.kind,
            provider: AccountsWidgetTimelineProvider(variant: .all)
        ) { entry in
            AccountsWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Copool Accounts")
        .description("See the current Codex and Claude accounts and remaining quota at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct CopoolCodexAccountsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(
            kind: AccountsWidgetVariant.codex.kind,
            provider: AccountsWidgetTimelineProvider(variant: .codex)
        ) { entry in
            AccountsWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Copool Codex Accounts")
        .description("See the current Codex account and remaining quota at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct CopoolClaudeAccountsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(
            kind: AccountsWidgetVariant.claude.kind,
            provider: AccountsWidgetTimelineProvider(variant: .claude)
        ) { entry in
            AccountsWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Copool Claude Accounts")
        .description("See the current Claude account and remaining quota at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct AccountsWidgetEntry: TimelineEntry {
    let date: Date
    let snapshot: AccountsWidgetSnapshot
}

struct AccountsWidgetTimelineProvider: TimelineProvider {
    let variant: AccountsWidgetVariant
    private let store: AccountsWidgetSnapshotStore

    init(variant: AccountsWidgetVariant) {
        self.variant = variant
        store = AccountsWidgetSnapshotStore(variant: variant)
    }

    func placeholder(in context: Context) -> AccountsWidgetEntry {
        AccountsWidgetEntry(date: .now, snapshot: sampleSnapshot)
    }

    func getSnapshot(in context: Context, completion: @escaping (AccountsWidgetEntry) -> Void) {
        let snapshot = store.load()
        completion(
            AccountsWidgetEntry(
                date: .now,
                snapshot: snapshot
            )
        )
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<AccountsWidgetEntry>) -> Void) {
        _ = context
        let snapshot = store.load()
        let entry = AccountsWidgetEntry(
            date: .now,
            snapshot: snapshot
        )
        completion(
            Timeline(
                entries: [entry],
                policy: .after(.now.addingTimeInterval(60 * 15))
            )
        )
    }

    /// The sample's pool and plan tags for the current card, the next and the two rows.
    private var samplePools: [String?] {
        variant == .all ? ["CODEX", "CLAUDE", "CLAUDE", "CODEX"] : [nil, nil, nil, nil]
    }

    private var samplePlans: [String] {
        switch variant {
        case .codex: ["TEAM", "PRO", "TEAM", "PLUS"]
        case .claude: ["MAX 20X", "PRO", "TEAM", "MAX 5X"]
        case .all: ["TEAM", "MAX 20X", "MAX 20X", "PLUS"]
        }
    }

    private var sampleSnapshot: AccountsWidgetSnapshot {
        AccountsWidgetSnapshot(
            generatedAt: Int64(Date().timeIntervalSince1970),
            usageProgressDisplayMode: .used,
            currentCard: AccountsWidgetCardSnapshot(
                id: "current",
                poolLabel: samplePools[0],
                planLabel: samplePlans[0],
                workspaceLabel: variant == .claude ? nil : "workspace",
                accountLabel: "current_account",
                fiveHour: AccountsWidgetWindowSnapshot(
                    title: "5h",
                    available: true,
                    progressFraction: 0.42,
                    usedText: "42%",
                    remainingText: "58%",
                    resetText: "26/3/21 01:23:45"
                ),
                oneWeek: AccountsWidgetWindowSnapshot(
                    title: "1w",
                    available: true,
                    progressFraction: 0.18,
                    usedText: "18%",
                    remainingText: "82%",
                    resetText: "26/3/27 01:23:45"
                )
            ),
            secondaryCard: AccountsWidgetCardSnapshot(
                id: "next",
                poolLabel: samplePools[1],
                planLabel: samplePlans[1],
                workspaceLabel: nil,
                accountLabel: "next_account",
                fiveHour: AccountsWidgetWindowSnapshot(
                    title: "5h",
                    available: true,
                    progressFraction: 0.55,
                    usedText: "55%",
                    remainingText: "45%",
                    resetText: "26/3/21 03:11:22"
                ),
                oneWeek: AccountsWidgetWindowSnapshot(
                    title: "1w",
                    available: true,
                    progressFraction: 0.31,
                    usedText: "31%",
                    remainingText: "69%",
                    resetText: "26/3/26 11:04:12"
                )
            ),
            rows: [
                AccountsWidgetRowSnapshot(
                    id: "row-1",
                    poolLabel: samplePools[2],
                    planLabel: samplePlans[2],
                    workspaceLabel: variant == .all ? nil : "abcdefg",
                    accountLabel: "account_name",
                    fiveHour: AccountsWidgetWindowSnapshot(
                        title: "5h",
                        available: true,
                        progressFraction: 0.50,
                        usedText: "50%",
                        remainingText: "50%",
                        resetText: "26/3/21 01:23:45"
                    ),
                    oneWeek: AccountsWidgetWindowSnapshot(
                        title: "1w",
                        available: true,
                        progressFraction: 0.02,
                        usedText: "2%",
                        remainingText: "98%",
                        resetText: "26/3/27 01:23:45"
                    )
                ),
                AccountsWidgetRowSnapshot(
                    id: "row-2",
                    poolLabel: samplePools[3],
                    planLabel: samplePlans[3],
                    workspaceLabel: nil,
                    accountLabel: "plus_account",
                    fiveHour: AccountsWidgetWindowSnapshot(
                        title: "5h",
                        available: true,
                        progressFraction: 0.18,
                        usedText: "18%",
                        remainingText: "82%",
                        resetText: "26/3/21 08:54:12"
                    ),
                    oneWeek: AccountsWidgetWindowSnapshot(
                        title: "1w",
                        available: true,
                        progressFraction: 0.36,
                        usedText: "36%",
                        remainingText: "64%",
                        resetText: "26/3/25 18:22:09"
                    )
                ),
            ]
        )
    }
}
