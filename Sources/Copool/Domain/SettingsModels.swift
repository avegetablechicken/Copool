import Foundation

enum UsageProgressDisplayMode: String, Codable, Equatable, CaseIterable, Sendable {
    case used
    case remaining

    var localizationKey: String {
        switch self {
        case .used:
            return "settings.usage_progress_display.used"
        case .remaining:
            return "settings.usage_progress_display.remaining"
        }
    }
}

struct Sub2APIProviderConfiguration: Codable, Equatable, Sendable {
    var isEnabled: Bool
    var providerID: String
    var adminBaseURL: String
    var username: String
    var password: String
    var allowInsecureTLS: Bool
    var importedAccountIDs: [Int64]
    var confirmedProviderIDs: [String]
    var cachedAccounts: [Sub2APIAccountSummary]

    enum CodingKeys: String, CodingKey {
        case isEnabled
        case providerID
        case adminBaseURL
        case username
        case password
        case allowInsecureTLS
        case importedAccountIDs
        case confirmedProviderIDs
        case cachedAccounts
    }

    init(
        isEnabled: Bool = false,
        providerID: String = "",
        adminBaseURL: String = "",
        username: String = "",
        password: String = "",
        allowInsecureTLS: Bool = false,
        importedAccountIDs: [Int64] = [],
        confirmedProviderIDs: [String] = [],
        cachedAccounts: [Sub2APIAccountSummary] = []
    ) {
        self.isEnabled = isEnabled
        self.providerID = providerID
        self.adminBaseURL = adminBaseURL
        self.username = username
        self.password = password
        self.allowInsecureTLS = allowInsecureTLS
        self.importedAccountIDs = importedAccountIDs
        self.confirmedProviderIDs = confirmedProviderIDs
        self.cachedAccounts = cachedAccounts
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        isEnabled = try container.decodeIfPresent(Bool.self, forKey: .isEnabled) ?? false
        providerID = try container.decodeIfPresent(String.self, forKey: .providerID) ?? ""
        adminBaseURL = try container.decodeIfPresent(String.self, forKey: .adminBaseURL) ?? ""
        username = try container.decodeIfPresent(String.self, forKey: .username) ?? ""
        password = try container.decodeIfPresent(String.self, forKey: .password) ?? ""
        allowInsecureTLS = try container.decodeIfPresent(Bool.self, forKey: .allowInsecureTLS) ?? false
        importedAccountIDs = try container.decodeIfPresent([Int64].self, forKey: .importedAccountIDs) ?? []
        confirmedProviderIDs = try container.decodeIfPresent([String].self, forKey: .confirmedProviderIDs) ?? []
        cachedAccounts = try container.decodeIfPresent([Sub2APIAccountSummary].self, forKey: .cachedAccounts) ?? []
    }

    func normalized() -> Sub2APIProviderConfiguration {
        var value = self
        value.providerID = providerID.trimmingCharacters(in: .whitespacesAndNewlines)
        value.adminBaseURL = adminBaseURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        value.username = username.trimmingCharacters(in: .whitespacesAndNewlines)
        value.isEnabled = !value.username.isEmpty && !value.password.isEmpty
        value.importedAccountIDs = Array(Set(importedAccountIDs.filter { $0 > 0 })).sorted()
        let importedIDs = Set(value.importedAccountIDs)
        value.cachedAccounts = cachedAccounts.reduce(into: [Sub2APIAccountSummary]()) { result, account in
            guard importedIDs.contains(account.id), !result.contains(where: { $0.id == account.id }) else {
                return
            }
            result.append(account)
        }
        let legacyProviderID = value.providerID
        value.providerID = ""
        value.confirmedProviderIDs = confirmedProviderIDs.reduce(into: [String]()) { result, rawValue in
            let providerID = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !providerID.isEmpty,
                  providerID.caseInsensitiveCompare(legacyProviderID) != .orderedSame,
                  !result.contains(where: { $0.caseInsensitiveCompare(providerID) == .orderedSame }) else {
                return
            }
            result.append(providerID)
        }
        return value
    }

    var isComplete: Bool {
        let value = normalized()
        return !value.username.isEmpty
            && !value.password.isEmpty
    }

    var associatedProviderIDs: [String] {
        normalized().confirmedProviderIDs
    }

    func confirms(providerID: String) -> Bool {
        associatedProviderIDs.contains {
            $0.caseInsensitiveCompare(providerID) == .orderedSame
        }
    }

    static let defaultValue = Sub2APIProviderConfiguration()
}

struct AppSettings: Codable, Equatable {
    var launchAtStartup: Bool
    var launchCodexAfterSwitch: Bool
    var autoSmartSwitch: Bool
    var syncOpencodeOpenaiAuth: Bool
    var localProxyHostAPIOnly: Bool
    var restartEditorsOnSwitch: Bool
    var restartEditorTargets: [EditorAppID]
    var autoStartApiProxy: Bool
    var proxyConfiguration: ProxyConfiguration
    var remoteServers: [RemoteServerConfig]
    var usageProgressDisplayMode: UsageProgressDisplayMode
    var sub2APIProvider: Sub2APIProviderConfiguration
    var locale: String

    enum CodingKeys: String, CodingKey {
        case launchAtStartup
        case launchCodexAfterSwitch
        case autoSmartSwitch
        case syncOpencodeOpenaiAuth
        case localProxyHostAPIOnly
        case restartEditorsOnSwitch
        case restartEditorTargets
        case autoStartApiProxy
        case proxyConfiguration
        case remoteServers
        case usageProgressDisplayMode
        case sub2APIProvider
        case locale
    }

    init(
        launchAtStartup: Bool,
        launchCodexAfterSwitch: Bool,
        autoSmartSwitch: Bool,
        syncOpencodeOpenaiAuth: Bool,
        localProxyHostAPIOnly: Bool = false,
        restartEditorsOnSwitch: Bool,
        restartEditorTargets: [EditorAppID],
        autoStartApiProxy: Bool,
        proxyConfiguration: ProxyConfiguration = .defaultValue,
        remoteServers: [RemoteServerConfig],
        usageProgressDisplayMode: UsageProgressDisplayMode = .used,
        sub2APIProvider: Sub2APIProviderConfiguration = .defaultValue,
        locale: String
    ) {
        self.launchAtStartup = launchAtStartup
        self.launchCodexAfterSwitch = launchCodexAfterSwitch
        self.autoSmartSwitch = autoSmartSwitch
        self.syncOpencodeOpenaiAuth = syncOpencodeOpenaiAuth
        self.localProxyHostAPIOnly = localProxyHostAPIOnly
        self.restartEditorsOnSwitch = restartEditorsOnSwitch
        self.restartEditorTargets = restartEditorTargets
        self.autoStartApiProxy = autoStartApiProxy
        self.proxyConfiguration = proxyConfiguration.normalized()
        self.remoteServers = remoteServers
        self.usageProgressDisplayMode = usageProgressDisplayMode
        self.sub2APIProvider = sub2APIProvider.normalized()
        self.locale = AppLocale.resolve(locale).identifier
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        launchAtStartup = try container.decode(Bool.self, forKey: .launchAtStartup)
        launchCodexAfterSwitch = try container.decode(Bool.self, forKey: .launchCodexAfterSwitch)
        autoSmartSwitch = try container.decode(Bool.self, forKey: .autoSmartSwitch)
        syncOpencodeOpenaiAuth = try container.decode(Bool.self, forKey: .syncOpencodeOpenaiAuth)
        localProxyHostAPIOnly = try container.decode(Bool.self, forKey: .localProxyHostAPIOnly)
        restartEditorsOnSwitch = try container.decode(Bool.self, forKey: .restartEditorsOnSwitch)
        restartEditorTargets = try container.decode([EditorAppID].self, forKey: .restartEditorTargets)
        autoStartApiProxy = try container.decode(Bool.self, forKey: .autoStartApiProxy)
        proxyConfiguration = try container.decode(ProxyConfiguration.self, forKey: .proxyConfiguration)
        remoteServers = try container.decode([RemoteServerConfig].self, forKey: .remoteServers)
        usageProgressDisplayMode = try container.decodeIfPresent(
            UsageProgressDisplayMode.self,
            forKey: .usageProgressDisplayMode
        ) ?? .used
        sub2APIProvider = try container.decodeIfPresent(
            Sub2APIProviderConfiguration.self,
            forKey: .sub2APIProvider
        )?.normalized() ?? .defaultValue
        locale = AppLocale.resolve(try container.decode(String.self, forKey: .locale)).identifier
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(launchAtStartup, forKey: .launchAtStartup)
        try container.encode(launchCodexAfterSwitch, forKey: .launchCodexAfterSwitch)
        try container.encode(autoSmartSwitch, forKey: .autoSmartSwitch)
        try container.encode(syncOpencodeOpenaiAuth, forKey: .syncOpencodeOpenaiAuth)
        try container.encode(localProxyHostAPIOnly, forKey: .localProxyHostAPIOnly)
        try container.encode(restartEditorsOnSwitch, forKey: .restartEditorsOnSwitch)
        try container.encode(restartEditorTargets, forKey: .restartEditorTargets)
        try container.encode(autoStartApiProxy, forKey: .autoStartApiProxy)
        try container.encode(proxyConfiguration, forKey: .proxyConfiguration)
        try container.encode(remoteServers, forKey: .remoteServers)
        try container.encode(usageProgressDisplayMode, forKey: .usageProgressDisplayMode)
        try container.encode(sub2APIProvider.normalized(), forKey: .sub2APIProvider)
        try container.encode(locale, forKey: .locale)
    }

    static var defaultValue: AppSettings {
        AppSettings(
            launchAtStartup: false,
            launchCodexAfterSwitch: true,
            autoSmartSwitch: false,
            syncOpencodeOpenaiAuth: false,
            localProxyHostAPIOnly: false,
            restartEditorsOnSwitch: false,
            restartEditorTargets: [],
            autoStartApiProxy: false,
            proxyConfiguration: .defaultValue,
            remoteServers: [],
            usageProgressDisplayMode: .used,
            sub2APIProvider: .defaultValue,
            locale: AppLocale.systemDefault.identifier
        )
    }
}

struct AppSettingsPatch {
    var launchAtStartup: Bool? = nil
    var launchCodexAfterSwitch: Bool? = nil
    var autoSmartSwitch: Bool? = nil
    var syncOpencodeOpenaiAuth: Bool? = nil
    var localProxyHostAPIOnly: Bool? = nil
    var restartEditorsOnSwitch: Bool? = nil
    var restartEditorTargets: [EditorAppID]? = nil
    var autoStartApiProxy: Bool? = nil
    var proxyConfiguration: ProxyConfiguration? = nil
    var remoteServers: [RemoteServerConfig]? = nil
    var usageProgressDisplayMode: UsageProgressDisplayMode? = nil
    var sub2APIProvider: Sub2APIProviderConfiguration? = nil
    var locale: String? = nil
}
