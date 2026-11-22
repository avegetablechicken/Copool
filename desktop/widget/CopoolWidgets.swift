import SwiftUI
import WidgetKit

@main
struct CopoolWidgets: WidgetBundle {
    var body: some Widget {
        CopoolAccountsWidget()
        CopoolCodexAccountsWidget()
        CopoolClaudeAccountsWidget()
        CopoolAntigravityAccountsWidget()
        CopoolGrokAccountsWidget()
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
        .description("See the current Codex, Claude, Antigravity and Grok accounts and remaining quota at a glance.")
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

struct CopoolAntigravityAccountsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(
            kind: AccountsWidgetVariant.antigravity.kind,
            provider: AccountsWidgetTimelineProvider(variant: .antigravity)
        ) { entry in
            AccountsWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Copool Antigravity Accounts")
        .description("See the current Antigravity account and remaining Gemini quota at a glance.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct CopoolGrokAccountsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(
            kind: AccountsWidgetVariant.grok.kind,
            provider: AccountsWidgetTimelineProvider(variant: .grok)
        ) { entry in
            AccountsWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Copool Grok Accounts")
        .description("See the current Grok account and remaining weekly quota at a glance.")
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
        case .codex: ["TEAM", "PRO 20X", "TEAM", "PLUS"]
        case .claude: ["ULTRA", "PRO", "TEAM", "MAX"]
        case .antigravity: ["PRO", "ULTRA 20X", "PRO", "FREE"]
        case .grok: ["SUPERGROK", "SUPERGROK", "SUPERGROK HEAVY", "FREE"]
        case .all: ["TEAM", "ULTRA", "ULTRA", "PLUS"]
        }
    }

    /// The sample plans' tier colors, as the app resolves them.
    private var samplePlanAccents: [String] {
        switch variant {
        case .codex: ["teal", "orange", "teal", "pink"]
        case .claude: ["orange", "pink", "teal", "orange"]
        case .antigravity: ["pink", "orange", "pink", "gray"]
        case .grok: ["pink", "pink", "orange", "gray"]
        case .all: ["teal", "orange", "orange", "pink"]
        }
    }

    /// A sample card's five-hour window; xAI reports none for Grok.
    private func sampleFiveHour(_ window: AccountsWidgetWindowSnapshot) -> AccountsWidgetWindowSnapshot {
        guard variant == .grok else { return window }
        return AccountsWidgetWindowSnapshot(
            title: window.title,
            available: false,
            progressFraction: 0,
            usedText: "--",
            remainingText: "--",
            resetText: "--"
        )
    }

    private var sampleSnapshot: AccountsWidgetSnapshot {
        AccountsWidgetSnapshot(
            generatedAt: Int64(Date().timeIntervalSince1970),
            usageProgressDisplayMode: .used,
            currentCard: AccountsWidgetCardSnapshot(
                id: "current",
                poolLabel: samplePools[0],
                planLabel: samplePlans[0],
                planAccent: samplePlanAccents[0],
                workspaceLabel: variant == .codex || variant == .all ? "workspace" : nil,
                accountLabel: "current_account",
                fiveHour: sampleFiveHour(AccountsWidgetWindowSnapshot(
                    title: "5h",
                    available: true,
                    progressFraction: 0.42,
                    usedText: "42%",
                    remainingText: "58%",
                    resetText: "26/3/21 01:23:45"
                )),
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
                planAccent: samplePlanAccents[1],
                workspaceLabel: nil,
                accountLabel: "next_account",
                fiveHour: sampleFiveHour(AccountsWidgetWindowSnapshot(
                    title: "5h",
                    available: true,
                    progressFraction: 0.55,
                    usedText: "55%",
                    remainingText: "45%",
                    resetText: "26/3/21 03:11:22"
                )),
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
                    planAccent: samplePlanAccents[2],
                    workspaceLabel: variant == .codex || variant == .claude ? "abcdefg" : nil,
                    accountLabel: "account_name",
                    fiveHour: sampleFiveHour(AccountsWidgetWindowSnapshot(
                        title: "5h",
                        available: true,
                        progressFraction: 0.50,
                        usedText: "50%",
                        remainingText: "50%",
                        resetText: "26/3/21 01:23:45"
                    )),
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
                    planAccent: samplePlanAccents[3],
                    workspaceLabel: nil,
                    accountLabel: "plus_account",
                    fiveHour: sampleFiveHour(AccountsWidgetWindowSnapshot(
                        title: "5h",
                        available: true,
                        progressFraction: 0.18,
                        usedText: "18%",
                        remainingText: "82%",
                        resetText: "26/3/21 08:54:12"
                    )),
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
