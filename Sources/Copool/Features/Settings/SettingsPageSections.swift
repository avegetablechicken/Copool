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
                SettingsLanguageSection(model: model)
                SettingsSwitchBehaviorSection(model: model)
                SettingsSub2APISection(model: model)
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
    @State private var expandedConfigurationIDs: Set<UUID> = []

    var body: some View {
        Group {
            Section("settings.section.sub2api") {
                HStack(spacing: 10) {
                    Image(systemName: "server.rack")
                        .foregroundStyle(.tint)
                    Text("Sub2api")
                        .font(.headline)
                    Spacer(minLength: 0)
                    Text(String(model.sub2APIProviderDraft.providers.count))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }

            ForEach($model.sub2APIProviderDraft.providers) { $configuration in
                Section {
                    HStack(spacing: 8) {
                        Button {
                            toggleConfiguration(configuration.id)
                        } label: {
                            HStack(spacing: 8) {
                                Image(
                                    systemName: expandedConfigurationIDs.contains(configuration.id)
                                        ? "chevron.down"
                                        : "chevron.right"
                                )
                                .font(.caption.weight(.semibold))
                                .frame(width: 12)
                                Rectangle()
                                    .fill(Color.accentColor)
                                    .frame(width: 3, height: 18)
                                Image(systemName: "server.rack")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text(configurationTitle(configuration.providerID))
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)

                        Button(role: .destructive) {
                            expandedConfigurationIDs.remove(configuration.id)
                            model.removeSub2APIProviderConfiguration(id: configuration.id)
                        } label: {
                            Image(systemName: "trash")
                        }
                        .buttonStyle(.plain)
                        .help(L10n.tr("settings.sub2api.remove_provider"))
                    }
                    .padding(.leading, 12)

                    if expandedConfigurationIDs.contains(configuration.id) {
                        TextField(
                            "settings.sub2api.provider_id",
                            text: $configuration.providerID,
                            prompt: Text("settings.sub2api.provider_id_placeholder")
                        )
                        .textFieldStyle(.roundedBorder)

                        TextField(
                            "settings.sub2api.username",
                            text: $configuration.username,
                            prompt: Text("settings.sub2api.username_placeholder")
                        )
                        .textContentType(.username)
                        .textFieldStyle(.roundedBorder)

                        SecureField(
                            "settings.sub2api.password",
                            text: $configuration.password
                        )
                        .textContentType(.password)
                        .textFieldStyle(.roundedBorder)

                        Toggle(
                            "settings.sub2api.allow_insecure_tls",
                            isOn: $configuration.allowInsecureTLS
                        )
                        .toggleStyle(.switch)
                    }
                }
                .padding(.vertical, -4)
            }

            Section {
                HStack {
                    Button(action: model.addSub2APIProviderConfiguration) {
                        Image(systemName: "plus")
                    }
                    .buttonStyle(.plain)
                    .help(L10n.tr("settings.sub2api.add_provider"))

                    Spacer(minLength: 0)
                    Button(action: model.saveSub2APIProvider) {
                        Label("common.save", systemImage: "square.and.arrow.down")
                    }
                    .copoolActionButtonStyle(prominent: true)
                    .disabled(model.isSavingSub2APIProvider)
                }
            }
        }
    }

    private func toggleConfiguration(_ id: UUID) {
        withAnimation(.easeInOut(duration: 0.18)) {
            if expandedConfigurationIDs.contains(id) {
                expandedConfigurationIDs.remove(id)
            } else {
                expandedConfigurationIDs.insert(id)
            }
        }
    }

    private func configurationTitle(_ providerID: String) -> String {
        let providerID = providerID.trimmingCharacters(in: .whitespacesAndNewlines)
        return providerID.isEmpty ? "Sub2api" : providerID
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
