import SwiftUI

struct AccountCardView: View {
    let card: AccountCardViewState
    let onSwitch: () -> Void
    let onRefresh: () -> Void
    let onReauthenticate: () -> Void
    let onDelete: () -> Void
    let onSaveProxy: ((String) async throws -> Void)?

    @Environment(\.presentAccountProxyEditor) private var presentProxyEditor

    @State private var isHoveringCollapsedSwitch = false
    @State private var isCollapsedSwitchOverlayPresented = false

    init(
        card: AccountCardViewState,
        onSwitch: @escaping () -> Void,
        onRefresh: @escaping () -> Void,
        onReauthenticate: @escaping () -> Void,
        onDelete: @escaping () -> Void,
        onSaveProxy: ((String) async throws -> Void)? = nil
    ) {
        self.card = card
        self.onSwitch = onSwitch
        self.onRefresh = onRefresh
        self.onReauthenticate = onReauthenticate
        self.onDelete = onDelete
        self.onSaveProxy = onSaveProxy
    }

    private var palette: AccountCardPalette {
        AccountCardPalette(accent: card.presentation.accent, isCurrent: card.account.isCurrent)
    }

    private var interactionPresentation: AccountCardInteractionPresentation {
        AccountCardInteractionPresentation(
            isCollapsed: card.isCollapsed,
            isCurrent: card.account.isCurrent,
            switching: card.switching,
            isHoveringCollapsedSwitch: isHoveringCollapsedSwitch,
            isCollapsedSwitchOverlayPresented: isCollapsedSwitchOverlayPresented,
            platform: accountCardInteractionPlatform
        )
    }

    private var presentation: AccountCardPresentation {
        card.presentation
    }

    private var accountCardInteractionPlatform: AccountCardInteractionPlatform {
        .macOS
    }

    var body: some View {
        cardBody
            .contextMenu {
                if onSaveProxy != nil {
                    Button("accounts.proxy.title", systemImage: "network", action: editProxy)
                }
            }
            .copoolCollapsedSwitchHover(
                enabled: interactionPresentation.canHoverSwitchOverlay,
                isHoveringCollapsedSwitch: $isHoveringCollapsedSwitch
            )
            .onChange(of: card.isCollapsed) { _, collapsed in
                if !collapsed {
                    dismissCollapsedSwitchOverlay()
                }
            }
            .onChange(of: card.account.isCurrent) { _, isCurrent in
                if isCurrent {
                    dismissCollapsedSwitchOverlay()
                }
            }
    }

    @ViewBuilder
    private var cardBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            if card.isCollapsed {
                VStack(alignment: .leading, spacing: 8) {
                    AccountCompactHeaderContent(
                        planLabel: presentation.planLabel,
                        workspaceLabel: presentation.teamNameTag,
                        statusLabel: presentation.statusLabel,
                        accountName: presentation.displayAccountName,
                        accentColor: palette.toneColor,
                        titleFont: .headline,
                        titleColor: card.account.isCurrent ? palette.toneColor : .primary,
                        spacing: 8
                    )
                    AccountCardCompactUsageSection(presentation: presentation)
                }
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    AccountCardHeaderSection(
                        presentation: presentation,
                        isCollapsed: card.isCollapsed,
                        isCurrent: card.account.isCurrent,
                        palette: palette,
                        onDelete: onDelete,
                        onEditProxy: { editProxy() }
                    )

                    Text(presentation.displayAccountName)
                        .font(.headline)
                        .foregroundStyle(card.account.isCurrent ? palette.toneColor : .primary)
                        .lineLimit(1)
                        .truncationMode(.tail)

                    AccountCardExpandedUsageSection(presentation: presentation)
                }
            }
        }
        .padding(card.isCollapsed ? 8 : 10)
        .accountCardSurface(cornerRadius: 12, tint: palette.surfaceTint)
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(palette.selectionBorderColor ?? .clear, lineWidth: 1)
        )
        .overlay(alignment: .bottomTrailing) {
            AccountCardBottomOverlay(
                isCollapsed: card.isCollapsed,
                isCurrent: card.account.isCurrent,
                showsSwitchButton: true,
                switching: card.switching,
                refreshing: card.refreshing,
                showsRefreshButton: card.showsRefreshButton,
                showsReauthenticateButton: card.showsReauthenticateButton,
                isRefreshEnabled: card.isRefreshEnabled,
                usageError: card.isUsageRefreshActive ? nil : card.account.usageError,
                palette: palette,
                onSwitch: onSwitch,
                onRefresh: onRefresh,
                onReauthenticate: onReauthenticate
            )
        }
        .animation(AccountCardMorphRules.animation, value: card.isCollapsed)
        .animation(AccountCardMorphRules.animation, value: card.account.isCurrent)
        .overlay {
            AccountCollapsedSwitchOverlay(
                isVisible: interactionPresentation.isCollapsedSwitchOverlayVisible,
                switching: card.switching,
                onDismiss: dismissCollapsedSwitchOverlay,
                onSwitch: onSwitch
            )
        }
    }

    private func editProxy() {
        guard let onSaveProxy else { return }
        presentProxyEditor?(AccountProxyEditorRequest(
            accountName: presentation.displayAccountName,
            inheritsProvider: card.account.sourceTag == "SUB2API",
            proxyURL: card.account.proxyURL,
            onSave: onSaveProxy
        ))
    }

    private func dismissCollapsedSwitchOverlay() {
        guard isCollapsedSwitchOverlayPresented else { return }
        withAnimation(AccountsAnimationRules.cardHoverOverlay) {
            isCollapsedSwitchOverlayPresented = false
        }
    }
}

private struct CollapsedSwitchHoverModifier: ViewModifier {
    let enabled: Bool
    @Binding var isHoveringCollapsedSwitch: Bool

    func body(content: Content) -> some View {
        if enabled {
            content.onHover { hovering in
                guard isHoveringCollapsedSwitch != hovering else { return }
                withAnimation(AccountsAnimationRules.cardHoverOverlay) {
                    isHoveringCollapsedSwitch = hovering
                }
            }
        } else {
            content
                .onAppear {
                    isHoveringCollapsedSwitch = false
                }
        }
    }
}

private extension View {
    func copoolCollapsedSwitchHover(
        enabled: Bool,
        isHoveringCollapsedSwitch: Binding<Bool>
    ) -> some View {
        modifier(
            CollapsedSwitchHoverModifier(
                enabled: enabled,
                isHoveringCollapsedSwitch: isHoveringCollapsedSwitch
            )
        )
    }
}


struct AccountProxyEditorRequest: Identifiable {
    let id = UUID()
    let accountName: String
    let inheritsProvider: Bool
    let proxyURL: String
    let onSave: (String) async throws -> Void
}

private struct AccountProxyEditorPresentationKey: EnvironmentKey {
    static var defaultValue: (@MainActor (AccountProxyEditorRequest) -> Void)? { nil }
}

private extension EnvironmentValues {
    var presentAccountProxyEditor: (@MainActor (AccountProxyEditorRequest) -> Void)? {
        get { self[AccountProxyEditorPresentationKey.self] }
        set { self[AccountProxyEditorPresentationKey.self] = newValue }
    }
}

/// Keep the dialog inside the MenuBarExtra window; dismissing a native sheet can hide that window.
private struct AccountProxyEditorPresentationModifier: ViewModifier {
    @State private var request: AccountProxyEditorRequest?

    func body(content: Content) -> some View {
        content
            .environment(\.presentAccountProxyEditor, { request = $0 })
            .disabled(request != nil)
            .accessibilityHidden(request != nil)
            .overlay {
                if let request {
                    ZStack {
                        Color.black.opacity(0.18)
                            .ignoresSafeArea()
                            .contentShape(Rectangle())
                            .onTapGesture {}
                        AccountProxyEditor(
                            accountName: request.accountName,
                            inheritsProvider: request.inheritsProvider,
                            initialValue: request.proxyURL,
                            onSave: request.onSave,
                            onClose: { self.request = nil }
                        )
                        .frame(maxWidth: 420)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
                        .shadow(radius: 16)
                        .padding(24)
                        .id(request.id)
                    }
                }
            }
    }
}

extension View {
    func accountProxyEditorPresentation() -> some View {
        modifier(AccountProxyEditorPresentationModifier())
    }
}

private struct AccountProxyEditor: View {
    let accountName: String
    let inheritsProvider: Bool
    let onSave: (String) async throws -> Void
    let onClose: () -> Void
    @State private var value: String
    @State private var errorMessage: String?
    @State private var isSaving = false

    init(accountName: String, inheritsProvider: Bool, initialValue: String, onSave: @escaping (String) async throws -> Void, onClose: @escaping () -> Void) {
        self.accountName = accountName
        self.inheritsProvider = inheritsProvider
        self.onSave = onSave
        self.onClose = onClose
        _value = State(initialValue: initialValue)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("accounts.proxy.title").font(.headline)
            Text(accountName).foregroundStyle(.secondary).lineLimit(2)
            TextField("settings.sub2api.proxy_url", text: $value, prompt: Text("http://127.0.0.1:7890"))
                .textFieldStyle(.roundedBorder)
                .disabled(isSaving)
            Text(LocalizedStringKey(inheritsProvider ? "accounts.proxy.provider_help" : "settings.sub2api.proxy_help"))
                .font(.caption)
                .foregroundStyle(.secondary)
            if let errorMessage {
                Text(errorMessage).font(.caption).foregroundStyle(.red)
            }
            HStack {
                Button("common.cancel", action: onClose)
                    .keyboardShortcut(.cancelAction)
                    .disabled(isSaving)
                Spacer()
                Button("common.save") {
                    isSaving = true
                    Task {
                        defer { isSaving = false }
                        do {
                            _ = try ProviderProxySession.proxyConfiguration(for: value)
                            try await onSave(value)
                            onClose()
                        } catch {
                            errorMessage = error.localizedDescription
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(isSaving)
            }
        }
        .padding(24)
        .frame(idealWidth: 420)

    }
}
