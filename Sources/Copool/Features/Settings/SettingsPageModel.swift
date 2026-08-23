import Foundation
import Combine

@MainActor
final class SettingsPageModel: ObservableObject {
    let settingsCoordinator: SettingsCoordinator
    let editorAppService: EditorAppServiceProtocol
    let codexConfigPath: URL?
    let onSettingsUpdated: @MainActor (AppSettings) -> Void
    let onQuitRequested: @MainActor () -> Void

    private let noticeScheduler = NoticeAutoDismissScheduler()

    @Published var settings: AppSettings = .defaultValue
    @Published var installedEditorApps: [InstalledEditorApp] = []
    @Published var sub2APIProviderDraft: Sub2APISettingsConfiguration = .defaultValue
    @Published var defaultCodexProviderID = "openai"
    @Published var isSavingSub2APIProvider = false
    @Published var notice: NoticeMessage? {
        didSet {
            noticeScheduler.schedule(notice) { [weak self] in
                self?.notice = nil
            }
        }
    }

    var hasLoaded = false

    init(
        settingsCoordinator: SettingsCoordinator,
        editorAppService: EditorAppServiceProtocol,
        codexConfigPath: URL? = nil,
        onSettingsUpdated: @escaping @MainActor (AppSettings) -> Void = { _ in },
        onQuitRequested: @escaping @MainActor () -> Void = {}
    ) {
        self.settingsCoordinator = settingsCoordinator
        self.editorAppService = editorAppService
        self.codexConfigPath = codexConfigPath
        self.onSettingsUpdated = onSettingsUpdated
        self.onQuitRequested = onQuitRequested
    }
}
