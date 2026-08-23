import Foundation
import SwiftUI

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
            AccountSwitchDebugLog.write(
                "accountsPage.smartSwitch.begin",
                "before=\(AccountSwitchDebugLog.describe(accounts: accountsBefore))"
            )
            let sorted = AccountRanking.sortByRemaining(accountsBefore)
            guard let best = sorted.first else {
                notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.no_switch_target"))
                return
            }
            if best.isCurrent {
                notice = NoticeMessage(style: .info, text: L10n.tr("accounts.notice.already_best"))
                return
            }

            let switchResult = try await coordinator.switchAccountAndReload(id: best.id)
            let accounts = switchResult.accounts
            let selectedAccount = switchResult.selectedAccount
            AccountSwitchDebugLog.write(
                "accountsPage.smartSwitch.loaded",
                "selected=\(AccountSwitchDebugLog.describe(account: selectedAccount)) \(AccountSwitchDebugLog.describe(accounts: accounts))"
            )
            applyAccountsForAccountSwitch(accounts)
            await refreshPendingWorkspaceAuthorizations(from: accounts, preferredSourceAccountID: selectedAccount.id)
            publishLocalAccounts(accounts)
            var switchNotice = buildSwitchNotice(execution: switchResult.execution)
            switchNotice.text = L10n.tr("accounts.notice.smart_switched_prefix_format", selectedAccount.label, switchNotice.text)
            notice = switchNotice
        } catch {
            AccountSwitchDebugLog.write(
                "accountsPage.smartSwitch.error",
                "error=\(error.localizedDescription)"
            )
            notice = NoticeMessage(style: .error, text: error.localizedDescription)
        }
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
