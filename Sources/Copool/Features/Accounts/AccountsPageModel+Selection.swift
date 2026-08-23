import Foundation
import SwiftUI

private enum AccountsSmartSwitchTarget {
    case local(AccountSummary)
    case sub2API(Sub2APIAccountSummary, AccountSummary)

    var account: AccountSummary {
        switch self {
        case .local(let account):
            return account
        case .sub2API(_, let account):
            return account
        }
    }

    var label: String {
        switch self {
        case .local(let account):
            return account.label
        case .sub2API(let source, _):
            return source.displayEmail
        }
    }
}

extension AccountsPageModel {
    func switchAccount(id: String) async {
        AccountSwitchDebugLog.write(
            "accountsPage.switchAccount.begin",
            "requestedCardID=\(id) displayed=\(AccountSwitchDebugLog.describe(accounts: debugDisplayedAccounts()))"
        )
        withAccountsSwitchAnimation {
            switchingAccountID = id
        }
        defer {
            withAccountsSwitchAnimation {
                switchingAccountID = nil
            }
        }

        do {
            if runtimePlatform == .macOS, let codexModelProviderSwitchService {
                try codexModelProviderSwitchService.switchProvider(to: "openai")
                applyCodexModelProviderID("openai")
            }
            let switchResult = try await coordinator.switchAccountAndReload(id: id)
            let accounts = switchResult.accounts
            let selectedAccount = switchResult.selectedAccount
            AccountSwitchDebugLog.write(
                "accountsPage.switchAccount.loaded",
                "selected=\(AccountSwitchDebugLog.describe(account: selectedAccount)) \(AccountSwitchDebugLog.describe(accounts: accounts))"
            )
            applyAccountsForAccountSwitch(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: selectedAccount.id)
            publishLocalAccounts(accounts)
            notice = buildSwitchNotice(execution: switchResult.execution)
        } catch {
            AccountSwitchDebugLog.write(
                "accountsPage.switchAccount.error",
                "requestedCardID=\(id) error=\(error.localizedDescription)"
            )
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    func switchSub2APIProvider(account: Sub2APIAccountSummary) async {
        let switchProviderID = account.providerID ?? sub2APIAccountService?.configuredProviderID()
        guard let codexModelProviderSwitchService,
              let providerID = switchProviderID?.trimmingCharacters(in: .whitespacesAndNewlines),
              !providerID.isEmpty,
              switchingAccountID == nil else {
            return
        }

        withAccountsSwitchAnimation {
            switchingAccountID = account.cardID
        }
        defer {
            withAccountsSwitchAnimation {
                switchingAccountID = nil
            }
        }

        do {
            try codexModelProviderSwitchService.switchProvider(to: providerID)
            sub2APIAccounts = sub2APIAccounts.map {
                $0.cardID == account.cardID ? $0.settingProvider(providerID) : $0
            }
            publishSub2APIAccounts()
            try? await persistSub2APIAccountCache()
            applyCodexModelProviderID(providerID)
            notice = NoticeMessage(
                style: .success,
                text: L10n.tr("accounts.notice.provider_switched_format", providerID)
            )
        } catch {
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    private func applyCodexModelProviderID(_ providerID: String) {
        acceptExternalCodexModelProviderID(providerID)
        onCodexModelProviderChanged?(providerID)
    }

    func smartSwitch() async {
        do {
            let accountsBefore = try await coordinator.listAccounts()
            let targets = smartSwitchTargets(localAccounts: accountsBefore)
            AccountSwitchDebugLog.write(
                "accountsPage.smartSwitch.begin",
                "before=\(AccountSwitchDebugLog.describe(accounts: targets.map(\.account)))"
            )
            guard let best = targets.max(by: {
                AccountRanking.remainingScore(for: $0.account)
                    < AccountRanking.remainingScore(for: $1.account)
            }) else {
                notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.no_switch_target"))
                return
            }
            if best.account.isCurrent {
                notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.already_best"))
                return
            }

            switch best {
            case .local(let account):
                await switchAccount(id: account.id)
            case .sub2API(let account, _):
                await switchSub2APIProvider(account: account)
            }
            prefixSmartSwitchNotice(targetLabel: best.label)
        } catch {
            AccountSwitchDebugLog.write(
                "accountsPage.smartSwitch.error",
                "error=\(error.localizedDescription)"
            )
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
    }

    private func smartSwitchTargets(localAccounts: [AccountSummary]) -> [AccountsSmartSwitchTarget] {
        let isOpenAICurrent = currentCodexModelProviderID.caseInsensitiveCompare("openai") == .orderedSame
        var targets = localAccounts.map { account in
            var account = account
            account.isCurrent = account.isCurrent && isOpenAICurrent
            return AccountsSmartSwitchTarget.local(account)
        }

        guard codexModelProviderSwitchService != nil else {
            return targets
        }

        let accountsByProvider = Dictionary(grouping: sub2APIAccounts) {
            $0.providerID?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
        }
        for accounts in accountsByProvider.values {
            guard let bestSub2APIAccount = accounts.max(by: {
                AccountRanking.remainingScore(for: $0.accountSummary)
                    < AccountRanking.remainingScore(for: $1.accountSummary)
            }),
            let providerID = bestSub2APIAccount.providerID?.trimmingCharacters(in: .whitespacesAndNewlines),
            !providerID.isEmpty else {
                continue
            }
            var providerAccount = bestSub2APIAccount.accountSummary
            providerAccount.isCurrent = providerID.caseInsensitiveCompare(currentCodexModelProviderID) == .orderedSame
            targets.append(.sub2API(bestSub2APIAccount, providerAccount))
        }
        return targets
    }

    private func prefixSmartSwitchNotice(targetLabel: String) {
        guard var switchNotice = notice else { return }
        if case .error = switchNotice.style { return }
        switchNotice.text = L10n.tr(
            "accounts.notice.smart_switched_prefix_format",
            targetLabel,
            switchNotice.text
        )
        notice = switchNotice
    }

    func toggleAllAccountsCollapsed() {
        let localIDs: Set<String>
        if case .content(let accounts) = state {
            localIDs = Set(accounts.filter { !$0.isWorkspaceDeactivated }.map(\.id))
        } else {
            localIDs = []
        }
        let ids = localIDs
            .union(sub2APIAccounts.map(\.cardID))
        guard !ids.isEmpty else {
            collapsedAccountIDs = []
            return
        }
        collapsedAccountIDs = collapsedAccountIDs.isSuperset(of: ids) ? [] : ids
    }

    private func applyAccountsForAccountSwitch(_ accounts: [AccountSummary]) {
        withAccountsSwitchAnimation {
            applyAccounts(accounts)
        }
    }

    private func withAccountsSwitchAnimation(_ updates: () -> Void) {
        withAnimation(AccountsAnimationRules.contentReorder) {
            updates()
        }
    }
}
