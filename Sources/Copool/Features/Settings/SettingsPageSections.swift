import SwiftUI

struct SettingsPageContent: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        #if os(macOS)
        MacSettingsPageContent(model: model)
        #else
        IOSSettingsPageContent(model: model)
        #endif
    }
}

#if os(macOS)
private struct MacSettingsPageContent: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        VStack(spacing: 0) {
            Form {
                SettingsGeneralSection(model: model)
                SettingsSub2APISection(model: model)
                SettingsLanguageSection(model: model)
                SettingsSwitchBehaviorSection(model: model)
            }
            .formStyle(.grouped)
            .scrollIndicators(.hidden)

            SettingsQuitFooter(onQuit: model.quitApp)
        }
        .task {
            await model.loadIfNeeded()
        }
    }
}

private struct SettingsSub2APISection: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        Section("settings.section.sub2api") {
            LabeledContent("settings.sub2api.default_provider") {
                Text(model.defaultCodexProviderID)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }

            LabeledContent("settings.sub2api.confirmed_providers") {
                HStack(spacing: 8) {
                    Text(confirmedProvidersText)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    Button(action: model.clearSub2APIProviderAssociations) {
                        Image(systemName: "link.badge.minus")
                    }
                    .buttonStyle(.plain)
                    .disabled(model.sub2APIProviderDraft.associatedProviderIDs.isEmpty)
                    .help(L10n.tr("settings.sub2api.clear_confirmed_providers"))
                }
            }

            TextField(
                "settings.sub2api.admin_base_url",
                text: $model.sub2APIProviderDraft.adminBaseURL,
                prompt: Text("settings.sub2api.admin_base_url_placeholder")
            )
            .textFieldStyle(.roundedBorder)

            TextField(
                "settings.sub2api.username",
                text: $model.sub2APIProviderDraft.username,
                prompt: Text("settings.sub2api.username_placeholder")
            )
            .textContentType(.username)
            .textFieldStyle(.roundedBorder)

            SecureField(
                "settings.sub2api.password",
                text: $model.sub2APIProviderDraft.password,
                prompt: Text("settings.sub2api.password_placeholder")
            )
            .textContentType(.password)
            .textFieldStyle(.roundedBorder)

            Toggle(
                "settings.sub2api.allow_insecure_tls",
                isOn: $model.sub2APIProviderDraft.allowInsecureTLS
            )
            .toggleStyle(.switch)

            HStack {
                Spacer(minLength: 0)
                Button(action: model.saveSub2APIProvider) {
                    Label("common.save", systemImage: "square.and.arrow.down")
                }
                .copoolActionButtonStyle(prominent: true)
                .disabled(model.isSavingSub2APIProvider)
            }
        }
    }

    private var confirmedProvidersText: String {
        let providerIDs = model.sub2APIProviderDraft.associatedProviderIDs
        return providerIDs.isEmpty ? L10n.tr("common.none") : providerIDs.joined(separator: ", ")
    }
}

private struct SettingsGeneralSection: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        Section("settings.section.general") {
            SettingsToggleRows(
                descriptors: model.generalSectionPresentation.toggles,
                onChange: model.updateToggle
            )

            if let usageProgressDisplayPicker = model.generalSectionPresentation.usageProgressDisplayPicker {
                SettingsPickerRow(
                    descriptor: usageProgressDisplayPicker,
                    onSelect: model.updateUsageProgressDisplayMode
                )
            }
        }
    }
}

private struct SettingsSwitchBehaviorSection: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        Section("settings.section.switch_behavior") {
            SettingsToggleRows(
                descriptors: model.switchBehaviorSectionPresentation.toggles,
                onChange: model.updateToggle
            )

            SettingsPickerRow(
                descriptor: model.switchBehaviorSectionPresentation.restartEditorTargetPicker,
                onSelect: model.updateRestartEditorTarget
            )
        }
    }
}

private struct SettingsQuitFooter: View {
    let onQuit: () -> Void

    var body: some View {
        HStack(spacing: LayoutRules.listRowSpacing) {
            Spacer(minLength: 0)

            Button(role: .destructive) {
                onQuit()
            } label: {
                Text("common.quit")
            }
            .buttonStyle(.frostedCapsule(prominent: true, tint: .red))
        }
        .padding(.horizontal, LayoutRules.pagePadding)
        .padding(.top, 6)
        .padding(.bottom, 10)
    }
}
#endif

private struct IOSSettingsPageContent: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        Form {
            SettingsLanguageSection(model: model)
        }
        .formStyle(.grouped)
        .scrollIndicators(.hidden)
        .task {
            await model.loadIfNeeded()
        }
    }
}

private struct SettingsLanguageSection: View {
    @ObservedObject var model: SettingsPageModel

    var body: some View {
        Section("settings.section.language") {
            SettingsPickerRow(
                descriptor: model.languageSectionPresentation.picker,
                onSelect: model.updateLocale
            )
        }
    }
}
