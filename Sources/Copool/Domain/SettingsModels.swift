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

struct Sub2APIProviderConfiguration: Codable, Equatable, Identifiable, Sendable {
    var id: UUID
    var providerID: String
    var username: String
    var password: String
    var allowInsecureTLS: Bool
    var proxyURL: String
    var accountProxyURLs: [String: String]
    var importedAccountIDs: [Int64]
    var cachedAccounts: [Sub2APIAccountSummary]
    var legacyAdminBaseURL: String

    enum CodingKeys: String, CodingKey {
        case id
        case providerID
        case username
        case password
        case allowInsecureTLS
        case proxyURL
        case accountProxyURLs
        case importedAccountIDs
        case cachedAccounts
        case legacyAdminBaseURL
    }

    init(
        id: UUID = UUID(),
        providerID: String = "",
        username: String = "",
        password: String = "",
        allowInsecureTLS: Bool = false,
        proxyURL: String = "",
        accountProxyURLs: [String: String] = [:],
        importedAccountIDs: [Int64] = [],
        cachedAccounts: [Sub2APIAccountSummary] = [],
        legacyAdminBaseURL: String = ""
    ) {
        self.id = id
        self.providerID = providerID
        self.username = username
        self.password = password
        self.allowInsecureTLS = allowInsecureTLS
        self.proxyURL = proxyURL
        self.accountProxyURLs = accountProxyURLs
        self.importedAccountIDs = importedAccountIDs
        self.cachedAccounts = cachedAccounts
        self.legacyAdminBaseURL = legacyAdminBaseURL
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        providerID = try container.decodeIfPresent(String.self, forKey: .providerID) ?? ""
        username = try container.decodeIfPresent(String.self, forKey: .username) ?? ""
        password = try container.decodeIfPresent(String.self, forKey: .password) ?? ""
        allowInsecureTLS = try container.decodeIfPresent(Bool.self, forKey: .allowInsecureTLS) ?? false
        proxyURL = try container.decodeIfPresent(String.self, forKey: .proxyURL) ?? ""
        accountProxyURLs = try container.decodeIfPresent([String: String].self, forKey: .accountProxyURLs) ?? [:]
        importedAccountIDs = try container.decodeIfPresent([Int64].self, forKey: .importedAccountIDs) ?? []
        cachedAccounts = try container.decodeIfPresent([Sub2APIAccountSummary].self, forKey: .cachedAccounts) ?? []
        legacyAdminBaseURL = try container.decodeIfPresent(String.self, forKey: .legacyAdminBaseURL) ?? ""
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(providerID, forKey: .providerID)
        try container.encode(username, forKey: .username)
        try container.encode(allowInsecureTLS, forKey: .allowInsecureTLS)
        try container.encode(proxyURL, forKey: .proxyURL)
        try container.encode(accountProxyURLs, forKey: .accountProxyURLs)
        try container.encode(importedAccountIDs, forKey: .importedAccountIDs)
        try container.encode(cachedAccounts, forKey: .cachedAccounts)
        try container.encode(legacyAdminBaseURL, forKey: .legacyAdminBaseURL)
    }

    func normalized() -> Sub2APIProviderConfiguration {
        var value = self
        value.providerID = providerID.trimmingCharacters(in: .whitespacesAndNewlines)
        value.proxyURL = proxyURL.trimmingCharacters(in: .whitespacesAndNewlines)
        value.username = username.trimmingCharacters(in: .whitespacesAndNewlines)
        value.legacyAdminBaseURL = legacyAdminBaseURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        value.importedAccountIDs = Array(Set(importedAccountIDs.filter { $0 > 0 })).sorted()
        let importedIDs = Set(value.importedAccountIDs)
        value.cachedAccounts = cachedAccounts.reduce(into: [Sub2APIAccountSummary]()) { result, account in
            guard importedIDs.contains(account.id), !result.contains(where: { $0.id == account.id }) else {
                return
            }
            var account = account
            account.proxyURL = value.accountProxyURLs[String(account.id)]
            result.append(account)
        }
        return value
    }

    func proxyURL(forAccountID accountID: Int64) -> String {
        let override = accountProxyURLs[String(accountID)]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return override.isEmpty ? proxyURL : override
    }

    var isComplete: Bool {
        let value = normalized()
        return !value.username.isEmpty
            && !value.password.isEmpty
    }

    var isEnabled: Bool {
        let value = normalized()
        return !value.providerID.isEmpty && value.isComplete
    }

    var hasEditableContent: Bool {
        let value = normalized()
        return !value.providerID.isEmpty
            || !value.proxyURL.isEmpty
            || !value.username.isEmpty
            || !value.password.isEmpty
            || !value.importedAccountIDs.isEmpty
            || !value.cachedAccounts.isEmpty
            || !value.legacyAdminBaseURL.isEmpty
    }
}

struct Sub2APISettingsConfiguration: Codable, Equatable, Sendable {
    var confirmedProviderIDs: [String]
    var providers: [Sub2APIProviderConfiguration]

    enum CodingKeys: String, CodingKey {
        case confirmedProviderIDs
        case providers
        case providerID
        case adminBaseURL
        case username
        case password
        case allowInsecureTLS
        case importedAccountIDs
        case cachedAccounts
    }

    init(
        confirmedProviderIDs: [String] = [],
        providers: [Sub2APIProviderConfiguration] = []
    ) {
        self.confirmedProviderIDs = confirmedProviderIDs
        self.providers = providers
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        confirmedProviderIDs = try container.decodeIfPresent(
            [String].self,
            forKey: .confirmedProviderIDs
        ) ?? []
        if let providers = try container.decodeIfPresent(
            [Sub2APIProviderConfiguration].self,
            forKey: .providers
        ) {
            self.providers = providers
            return
        }

        let legacyProvider = Sub2APIProviderConfiguration(
            providerID: try container.decodeIfPresent(String.self, forKey: .providerID) ?? "",
            username: try container.decodeIfPresent(String.self, forKey: .username) ?? "",
            password: try container.decodeIfPresent(String.self, forKey: .password) ?? "",
            allowInsecureTLS: try container.decodeIfPresent(Bool.self, forKey: .allowInsecureTLS) ?? false,
            importedAccountIDs: try container.decodeIfPresent([Int64].self, forKey: .importedAccountIDs) ?? [],
            cachedAccounts: try container.decodeIfPresent(
                [Sub2APIAccountSummary].self,
                forKey: .cachedAccounts
            ) ?? [],
            legacyAdminBaseURL: try container.decodeIfPresent(String.self, forKey: .adminBaseURL) ?? ""
        )
        providers = legacyProvider.hasEditableContent ? [legacyProvider] : []
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(confirmedProviderIDs, forKey: .confirmedProviderIDs)
        try container.encode(providers, forKey: .providers)
    }

    func normalized() -> Sub2APISettingsConfiguration {
        var value = self
        value.confirmedProviderIDs = confirmedProviderIDs.reduce(into: []) { result, rawValue in
            let providerID = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !providerID.isEmpty,
                  !result.contains(where: { $0.caseInsensitiveCompare(providerID) == .orderedSame }) else {
                return
            }
            result.append(providerID)
        }
        value.providers = providers.reduce(into: []) { result, rawConfiguration in
            let configuration = rawConfiguration.normalized()
            guard configuration.hasEditableContent else { return }
            if !configuration.providerID.isEmpty,
               result.contains(where: {
                   $0.providerID.caseInsensitiveCompare(configuration.providerID) == .orderedSame
               }) {
                return
            }
            result.append(configuration)
        }
        return value
    }

    var associatedProviderIDs: [String] {
        normalized().confirmedProviderIDs
    }

    func confirms(providerID: String) -> Bool {
        associatedProviderIDs.contains {
            $0.caseInsensitiveCompare(providerID) == .orderedSame
        }
    }

    func provider(for providerID: String) -> Sub2APIProviderConfiguration? {
        normalized().providers.first {
            $0.providerID.caseInsensitiveCompare(providerID) == .orderedSame
        }
    }

    func provider(containingAccountID accountID: Int64) -> Sub2APIProviderConfiguration? {
        normalized().providers.first { configuration in
            configuration.importedAccountIDs.contains(accountID)
                || configuration.cachedAccounts.contains(where: { $0.id == accountID })
        }
    }

    mutating func upsert(_ configuration: Sub2APIProviderConfiguration) {
        if let index = providers.firstIndex(where: { $0.id == configuration.id }) {
            providers[index] = configuration
        } else {
            providers.append(configuration)
        }
    }

    static let defaultValue = Sub2APISettingsConfiguration()
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
    var sub2APIProvider: Sub2APISettingsConfiguration
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
        sub2APIProvider: Sub2APISettingsConfiguration = .defaultValue,
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
            Sub2APISettingsConfiguration.self,
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
    var sub2APIProvider: Sub2APISettingsConfiguration? = nil
    var locale: String? = nil
}
