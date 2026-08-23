import Foundation
import OSLog
#if canImport(Security)
import Security
#endif

enum BackgroundNetworkSession {
    static let shared: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        return URLSession(configuration: configuration)
    }()

    static let insecureSub2API: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCache = nil
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        return URLSession(
            configuration: configuration,
            delegate: InsecureServerTrustSessionDelegate(),
            delegateQueue: nil
        )
    }()
}

private final class InsecureServerTrustSessionDelegate: NSObject, URLSessionDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        _ = session
        #if canImport(Security)
        if challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
           let trust = challenge.protectionSpace.serverTrust {
            completionHandler(.useCredential, URLCredential(trust: trust))
            return
        }
        #endif
        completionHandler(.performDefaultHandling, nil)
    }
}

final class DefaultUsageService: UsageService, @unchecked Sendable {
    private enum RequestPolicy {
        static let timeout: TimeInterval = 18
        static let scope = "usage"
    }

    private static let logger = Logger(subsystem: "Copool", category: "Usage")

    private let session: URLSession
    private let configPath: URL
    private let dateProvider: DateProviding
    private let endpointCoordinator: EndpointRequestCoordinator

    init(
        session: URLSession = BackgroundNetworkSession.shared,
        configPath: URL,
        dateProvider: DateProviding = SystemDateProvider(),
        endpointPreferenceStore: EndpointPreferenceStore = .shared
    ) {
        self.session = session
        self.configPath = configPath
        self.dateProvider = dateProvider
        self.endpointCoordinator = EndpointRequestCoordinator(
            session: session,
            preferenceStore: endpointPreferenceStore
        )
    }

    func fetchUsage(accessToken: String, accountID: String) async throws -> UsageSnapshot {
        let candidateURLs = resolveUsageURLs()
        let startedAt = Date()
        UsageDebugLog.write(
            "request.begin",
            "accountID=\(accountID) candidates=\(candidateURLs.joined(separator: " | "))"
        )
        // Self.logger.debug(
        //     "Usage request started for account \(accountID, privacy: .public). Candidates: \(candidateURLs.joined(separator: " | "), privacy: .public)"
        // )
        do {
            let resolved: ResolvedUsagePayload = try await endpointCoordinator.fetchFirstSuccessful(
                scope: RequestPolicy.scope,
                candidateURLs: candidateURLs
            ) { endpoint in
                var request = URLRequest(url: endpoint)
                request.timeoutInterval = RequestPolicy.timeout
                request.httpMethod = "GET"
                request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")
                request.setValue(accountID, forHTTPHeaderField: "ChatGPT-Account-Id")
                request.setValue("application/json", forHTTPHeaderField: "Accept")
                request.setValue("codex-tools-swift/0.1", forHTTPHeaderField: "User-Agent")
                // Self.logger.debug(
                //     "Usage request: \(Self.requestLogSummary(for: request), privacy: .public)"
                // )
                return request
            } validate: { result in
                // Self.logger.debug(
                //     "Usage raw response from \(result.endpoint, privacy: .public) [status \(result.response.statusCode)] for account \(accountID, privacy: .public): \(Self.responseLogBody(for: result.data), privacy: .public)"
                // )
                UsageDebugLog.write(
                    "response.raw",
                    "accountID=\(accountID) endpoint=\(result.endpoint) status=\(result.response.statusCode) body=\(Self.responseLogBody(for: result.data))"
                )
                return ResolvedUsagePayload(
                    endpoint: result.endpoint,
                    payload: try JSONDecoder().decode(UsageAPIResponse.self, from: result.data)
                )
            }
            // Self.logger.debug(
            //     "Usage request succeeded via \(resolved.endpoint, privacy: .public) in \(elapsedMilliseconds) ms for account \(accountID, privacy: .public)"
            // )
            let snapshot = Self.mapPayload(
                resolved.payload,
                fetchedAt: dateProvider.unixSecondsNow()
            )
            let elapsedMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            UsageDebugLog.write(
                "request.success",
                "accountID=\(accountID) endpoint=\(resolved.endpoint) elapsedMs=\(elapsedMilliseconds) usage=\(Self.describeUsage(snapshot))"
            )
            return snapshot
        } catch EndpointRequestError.allRequestsFailed(let errors) {
            let elapsedMilliseconds = Int(Date().timeIntervalSince(startedAt) * 1_000)
            Self.logger.error(
                "Usage request failed after \(elapsedMilliseconds) ms for account \(accountID, privacy: .public). Candidates: \(candidateURLs.joined(separator: " | "), privacy: .public). Errors: \(errors.joined(separator: " | "), privacy: .public)"
            )
            UsageDebugLog.write(
                "request.failure",
                "accountID=\(accountID) elapsedMs=\(elapsedMilliseconds) candidates=\(candidateURLs.joined(separator: " | ")) errors=\(errors.joined(separator: " | "))"
            )
            if let message = Self.preferredUserFacingFailureMessage(from: errors) {
                throw AppError.network(message)
            }
            let preview = errors.prefix(2).joined(separator: " | ")
            if errors.count > 2 {
                throw AppError.network(L10n.tr("error.usage.request_failed_with_more_format", preview, String(errors.count - 2)))
            }
            throw AppError.network(L10n.tr("error.usage.request_failed_format", preview))
        }
    }

    private static func preferredUserFacingFailureMessage(from errors: [String]) -> String? {
        for error in errors {
            let detail = error.components(separatedBy: ": ").dropFirst().joined(separator: ": ")
            guard let detail = detail.nonEmptyTrimmed, !detail.hasPrefix("<") else {
                continue
            }
            return detail
        }
        return nil
    }

    private static func requestLogSummary(for request: URLRequest) -> String {
        let method = request.httpMethod ?? "GET"
        let url = request.url?.absoluteString ?? ""
        let headers = (request.allHTTPHeaderFields ?? [:])
            .filter { $0.key.caseInsensitiveCompare("Authorization") != .orderedSame }
        let payload: [String: Any] = [
            "method": method,
            "url": url,
            "headers": headers
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8) else {
            return "\(method) \(url)"
        }
        return text
    }

    private static func responseLogBody(for data: Data) -> String {
        if let text = String(data: data, encoding: .utf8) {
            return text
        }
        return "<non-utf8 body: \(data.count) bytes>"
    }

    private func resolveUsageURLs() -> [String] {
        let baseOrigin = ChatGPTBaseOriginResolver.resolve(configPath: configPath)
        let backendPrefix = "/backend-api"
        let whamPath = "/wham/usage"
        let codexPath = "/api/codex/usage"

        var candidates: [String] = []
        if let originWithoutBackend = baseOrigin.removingSuffix(backendPrefix) {
            candidates.append("\(baseOrigin)\(whamPath)")
            candidates.append("\(originWithoutBackend)\(backendPrefix)\(whamPath)")
            candidates.append("\(originWithoutBackend)\(codexPath)")
        } else {
            candidates.append("\(baseOrigin)\(backendPrefix)\(whamPath)")
            candidates.append("\(baseOrigin)\(whamPath)")
            candidates.append("\(baseOrigin)\(codexPath)")
        }

        candidates.append("https://chatgpt.com/backend-api/wham/usage")
        candidates.append("https://chatgpt.com/api/codex/usage")

        var deduped: [String] = []
        for candidate in candidates where !deduped.contains(candidate) {
            deduped.append(candidate)
        }
        return deduped
    }

    fileprivate static func mapPayload(
        _ payload: UsageAPIResponse,
        fetchedAt: Int64
    ) -> UsageSnapshot {
        var windows: [UsageWindowRaw] = []

        if let rateLimit = payload.rateLimit {
            if let primary = rateLimit.primaryWindow { windows.append(primary) }
            if let secondary = rateLimit.secondaryWindow { windows.append(secondary) }
        }

        if let additional = payload.additionalRateLimits {
            for item in additional {
                if let primary = item.rateLimit?.primaryWindow { windows.append(primary) }
                if let secondary = item.rateLimit?.secondaryWindow { windows.append(secondary) }
            }
        }

        let fiveHourRaw = UsageWindowSelector.pickNearestWindow(windows, targetSeconds: 5 * 60 * 60)
        let oneWeekRaw = UsageWindowSelector.pickNearestWindow(windows, targetSeconds: 7 * 24 * 60 * 60)

        return UsageSnapshot(
            fetchedAt: fetchedAt,
            planType: payload.planType,
            fiveHour: fiveHourRaw.map(Self.toUsageWindow),
            oneWeek: oneWeekRaw.map(Self.toUsageWindow),
            credits: payload.credits.map {
                CreditSnapshot(hasCredits: $0.hasCredits, unlimited: $0.unlimited, balance: $0.balance)
            }
        )
    }

    private static func toUsageWindow(_ raw: UsageWindowRaw) -> UsageWindow {
        UsageWindow(
            usedPercent: raw.usedPercent,
            windowSeconds: raw.limitWindowSeconds,
            resetAt: raw.resetAt
        )
    }

    private static func describeUsage(_ usage: UsageSnapshot) -> String {
        "fetchedAt=\(usage.fetchedAt) fiveHourUsed=\(describePercent(usage.fiveHour?.usedPercent)) fiveHourReset=\(usage.fiveHour?.resetAt.map(String.init) ?? "nil") oneWeekUsed=\(describePercent(usage.oneWeek?.usedPercent)) oneWeekReset=\(usage.oneWeek?.resetAt.map(String.init) ?? "nil")"
    }

    private static func describePercent(_ value: Double?) -> String {
        guard let value else { return "nil" }
        return String(format: "%.2f", value)
    }
}

final class DefaultSub2APIAccountService: Sub2APIAccountServiceProtocol, @unchecked Sendable {
    private let configPath: URL
    private let settingsRepository: SettingsRepository
    private let secretStore: Sub2APISecretStoreProtocol?
    private let sub2APIClient: Sub2APIUsageClient
    private let dateProvider: DateProviding

    init(
        configPath: URL,
        settingsRepository: SettingsRepository,
        secretStore: Sub2APISecretStoreProtocol? = nil,
        session: URLSession = BackgroundNetworkSession.shared,
        insecureSession: URLSession = BackgroundNetworkSession.insecureSub2API,
        dateProvider: DateProviding = SystemDateProvider()
    ) {
        self.configPath = configPath
        self.settingsRepository = settingsRepository
        self.secretStore = secretStore
        self.dateProvider = dateProvider
        self.sub2APIClient = Sub2APIUsageClient(
            session: session,
            insecureSession: insecureSession
        )
    }

    func currentDefaultProviderID() -> String {
        CodexModelProviderResolver.resolve(configPath: configPath).id
    }

    func configuredProviderID() -> String? {
        guard let settings = try? settingsRepository.loadSettings().sub2APIProvider.normalized() else {
            return nil
        }
        let currentProviderID = currentDefaultProviderID()
        return settings.provider(for: currentProviderID)?.providerID
            ?? (settings.providers.count == 1 ? settings.providers[0].providerID : nil)
    }

    func isConnectionConfigured() -> Bool {
        guard let settings = try? settingsRepository.loadSettings().sub2APIProvider.normalized(),
              let storedConfiguration = settings.provider(for: currentDefaultProviderID()),
              let configuration = try? resolvedConfiguration(storedConfiguration) else {
            return false
        }
        return configuration.isEnabled
    }

    func canQueryCurrentDefaultProvider() -> Bool {
        guard let settings = try? settingsRepository.loadSettings().sub2APIProvider.normalized() else {
            return false
        }
        let provider = CodexModelProviderResolver.resolve(configPath: configPath)
        guard let storedConfiguration = settings.provider(for: provider.id),
              let configuration = try? resolvedConfiguration(storedConfiguration) else {
            return false
        }
        return configuration.isEnabled
            && !provider.isOfficialOpenAI
    }

    func fetchAccounts(accountIDs: [Int64]?) async throws -> [Sub2APIAccountSummary] {
        try await fetchAccounts(
            providerID: currentDefaultProviderID(),
            accountIDs: accountIDs
        )
    }

    func fetchAccounts(
        providerID: String,
        accountIDs: [Int64]?
    ) async throws -> [Sub2APIAccountSummary] {
        guard let route = try providerRoute(providerID: providerID) else {
            throw AppError.invalidData(L10n.tr("error.sub2api.provider_not_confirmed"))
        }
        let allAccounts = try await sub2APIClient.listOpenAIAccounts(
            configuration: route.configuration,
            provider: route.provider
        )
        let requestedIDs = accountIDs.map(Set.init)
        let accounts = allAccounts.filter { account in
            guard account.parentAccountID == nil,
                  account.platform.caseInsensitiveCompare("openai") == .orderedSame,
                  account.type.caseInsensitiveCompare("oauth") == .orderedSame,
                  account.status.caseInsensitiveCompare("active") == .orderedSame else {
                return false
            }
            guard let requestedIDs else { return true }
            return requestedIDs.contains(account.id)
        }
        let fetchedAt = dateProvider.unixSecondsNow()
        let accountProviderID = route.configuration.providerID

        return await withTaskGroup(of: Sub2APIAccountSummary.self) { group in
            for account in accounts {
                group.addTask { [sub2APIClient] in
                    do {
                        let result = try await sub2APIClient.fetchUsagePayload(
                            configuration: route.configuration,
                            provider: route.provider,
                            accountID: account.id,
                            fetchedAt: fetchedAt
                        )
                        return account.summary(
                            usage: result.usage,
                            email: result.email,
                            accountID: result.accountID,
                            usageError: nil,
                            providerID: accountProviderID
                        )
                    } catch {
                        return account.summary(
                            usage: nil,
                            email: nil,
                            accountID: nil,
                            usageError: error.localizedDescription,
                            providerID: accountProviderID
                        )
                    }
                }
            }

            var summaries: [Sub2APIAccountSummary] = []
            for await summary in group {
                summaries.append(summary)
            }
            return summaries.sorted {
                $0.displayEmail.localizedCaseInsensitiveCompare($1.displayEmail) == .orderedAscending
            }
        }
    }

    private func providerRoute(providerID: String) throws -> (
        configuration: Sub2APIProviderConfiguration,
        provider: CodexModelProviderDefinition
    )? {
        guard let settings = try? settingsRepository.loadSettings().sub2APIProvider.normalized() else {
            return nil
        }

        guard let provider = CodexModelProviderResolver.definitions(configPath: configPath).first(where: {
            $0.id.caseInsensitiveCompare(providerID) == .orderedSame
        }) else {
            return nil
        }
        guard let storedConfiguration = settings.provider(for: provider.id) else { return nil }
        let configuration = try resolvedConfiguration(storedConfiguration)
        guard !provider.isOfficialOpenAI else { return nil }
        guard configuration.isEnabled else {
            throw AppError.invalidData(L10n.tr("error.sub2api.configuration_incomplete"))
        }
        return (configuration, provider)
    }

    private func resolvedConfiguration(
        _ storedConfiguration: Sub2APIProviderConfiguration
    ) throws -> Sub2APIProviderConfiguration {
        var configuration = storedConfiguration
        if configuration.password.isEmpty, let secretStore {
            configuration.password = try secretStore.password(for: configuration.id) ?? ""
        }
        return configuration
    }
}

private actor Sub2APIUsageClient {
    private enum RequestPolicy {
        static let timeout: TimeInterval = 18
    }

    private struct CredentialKey: Hashable {
        var baseURL: String
        var username: String
        var password: String
        var allowInsecureTLS: Bool
    }

    private enum HTTPFailure: Error {
        case unauthorized(String)
        case other(String)
    }

    private let session: URLSession
    private let insecureSession: URLSession
    private var adminTokens: [CredentialKey: String] = [:]

    init(session: URLSession, insecureSession: URLSession) {
        self.session = session
        self.insecureSession = insecureSession
    }

    func listOpenAIAccounts(
        configuration: Sub2APIProviderConfiguration,
        provider: CodexModelProviderDefinition
    ) async throws -> [Sub2APIRemoteAccount] {
        let context = try await requestContext(configuration: configuration, provider: provider)
        do {
            return try await Self.requestAllOpenAIAccounts(
                baseURL: context.baseURL,
                token: context.token,
                session: context.session
            )
        } catch HTTPFailure.unauthorized {
            adminTokens[context.key] = nil
            let token = try await adminToken(
                key: context.key,
                baseURL: context.baseURL,
                configuration: context.configuration,
                session: context.session
            )
            do {
                return try await Self.requestAllOpenAIAccounts(
                    baseURL: context.baseURL,
                    token: token,
                    session: context.session
                )
            } catch HTTPFailure.unauthorized(let message) {
                throw AppError.unauthorized(L10n.tr("error.sub2api.request_failed_format", message))
            } catch HTTPFailure.other(let message) {
                throw AppError.network(L10n.tr("error.sub2api.request_failed_format", message))
            }
        } catch HTTPFailure.other(let message) {
            throw AppError.network(L10n.tr("error.sub2api.request_failed_format", message))
        }
    }

    func fetchUsagePayload(
        configuration: Sub2APIProviderConfiguration,
        provider: CodexModelProviderDefinition,
        accountID: Int64,
        fetchedAt: Int64
    ) async throws -> Sub2APIUsageResult {
        let context = try await requestContext(configuration: configuration, provider: provider)
        var token = context.token

        let payload: UsageAPIResponse
        do {
            payload = try await Self.requestQuota(
                baseURL: context.baseURL,
                accountID: accountID,
                token: token,
                session: context.session
            )
        } catch HTTPFailure.unauthorized {
            adminTokens[context.key] = nil
            token = try await adminToken(
                key: context.key,
                baseURL: context.baseURL,
                configuration: context.configuration,
                session: context.session
            )
            do {
                payload = try await Self.requestQuota(
                    baseURL: context.baseURL,
                    accountID: accountID,
                    token: token,
                    session: context.session
                )
            } catch HTTPFailure.other(let message) {
                throw AppError.network(L10n.tr("error.sub2api.request_failed_format", message))
            } catch HTTPFailure.unauthorized(let message) {
                throw AppError.unauthorized(
                    L10n.tr("error.sub2api.request_failed_format", message)
                )
            }
        } catch HTTPFailure.other(let message) {
            throw AppError.network(L10n.tr("error.sub2api.request_failed_format", message))
        }

        return Sub2APIUsageResult(
            usage: DefaultUsageService.mapPayload(payload, fetchedAt: fetchedAt),
            email: payload.email,
            accountID: payload.accountID
        )
    }

    private func requestContext(
        configuration: Sub2APIProviderConfiguration,
        provider: CodexModelProviderDefinition
    ) async throws -> (
        configuration: Sub2APIProviderConfiguration,
        baseURL: URL,
        key: CredentialKey,
        token: String,
        session: URLSession
    ) {
        let configuration = configuration.normalized()
        guard configuration.isComplete else {
            throw AppError.invalidData(L10n.tr("error.sub2api.configuration_incomplete"))
        }
        let baseURL = try Self.resolveAdminBaseURL(
            configuredValue: "",
            providerBaseURL: provider.baseURL
        )
        let key = CredentialKey(
            baseURL: baseURL.absoluteString,
            username: configuration.username,
            password: configuration.password,
            allowInsecureTLS: configuration.allowInsecureTLS
        )
        let requestSession = configuration.allowInsecureTLS ? insecureSession : session
        let token = try await adminToken(
            key: key,
            baseURL: baseURL,
            configuration: configuration,
            session: requestSession
        )
        return (configuration, baseURL, key, token, requestSession)
    }

    private func adminToken(
        key: CredentialKey,
        baseURL: URL,
        configuration: Sub2APIProviderConfiguration,
        session: URLSession
    ) async throws -> String {
        if let cached = adminTokens[key] {
            return cached
        }

        let token: String
        do {
            token = try await Self.login(
                baseURL: baseURL,
                username: configuration.username,
                password: configuration.password,
                session: session
            )
        } catch HTTPFailure.other(let message) {
            throw AppError.network(L10n.tr("error.sub2api.request_failed_format", message))
        } catch HTTPFailure.unauthorized(let message) {
            throw AppError.unauthorized(L10n.tr("error.sub2api.request_failed_format", message))
        }

        if let token = adminTokens[key] {
            return token
        }
        adminTokens[key] = token
        return token
    }

    private static func login(
        baseURL: URL,
        username: String,
        password: String,
        session: URLSession
    ) async throws -> String {
        let url = baseURL
            .appendingPathComponent("auth", isDirectory: true)
            .appendingPathComponent("login", isDirectory: false)
        var request = URLRequest(url: url)
        request.timeoutInterval = RequestPolicy.timeout
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "email": username,
            "password": password
        ])

        let (data, response) = try await perform(request, session: session)
        guard (200..<300).contains(response.statusCode) else {
            throw httpFailure(statusCode: response.statusCode, data: data)
        }
        if let code = APIResponseMessage.code(from: data), code != 0 {
            throw HTTPFailure.other(APIResponseMessage.message(from: data) ?? "API code \(code)")
        }
        guard let token = loginToken(from: data) else {
            if let message = APIResponseMessage.message(from: data) {
                throw HTTPFailure.other(message)
            }
            throw HTTPFailure.other(L10n.tr("error.sub2api.missing_token"))
        }
        return token
    }

    private static func requestAllOpenAIAccounts(
        baseURL: URL,
        token: String,
        session: URLSession
    ) async throws -> [Sub2APIRemoteAccount] {
        var page = 1
        var accounts: [Sub2APIRemoteAccount] = []

        while true {
            let response = try await requestOpenAIAccountPage(
                baseURL: baseURL,
                page: page,
                token: token,
                session: session
            )
            accounts.append(contentsOf: response.items)
            guard page < response.pages else { break }
            page += 1
        }
        return accounts
    }

    private static func requestOpenAIAccountPage(
        baseURL: URL,
        page: Int,
        token: String,
        session: URLSession
    ) async throws -> Sub2APIAccountListPage {
        let endpoint = baseURL
            .appendingPathComponent("admin", isDirectory: true)
            .appendingPathComponent("accounts", isDirectory: false)
        guard var components = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else {
            throw HTTPFailure.other(L10n.tr("error.sub2api.invalid_base_url"))
        }
        components.queryItems = [
            URLQueryItem(name: "page", value: String(page)),
            URLQueryItem(name: "page_size", value: "200"),
            URLQueryItem(name: "platform", value: "openai"),
            URLQueryItem(name: "type", value: "oauth"),
            URLQueryItem(name: "status", value: "active"),
            URLQueryItem(name: "lite", value: "1"),
            URLQueryItem(name: "sort_by", value: "name"),
            URLQueryItem(name: "sort_order", value: "asc")
        ]
        guard let url = components.url else {
            throw HTTPFailure.other(L10n.tr("error.sub2api.invalid_base_url"))
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = RequestPolicy.timeout
        request.httpMethod = "GET"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        let (data, response) = try await perform(request, session: session)
        guard (200..<300).contains(response.statusCode) else {
            throw httpFailure(statusCode: response.statusCode, data: data)
        }
        do {
            let envelope = try JSONDecoder().decode(Sub2APIAccountListEnvelope.self, from: data)
            guard envelope.code == 0 else {
                throw HTTPFailure.other(envelope.message ?? "API code \(envelope.code)")
            }
            guard let page = envelope.data else {
                throw HTTPFailure.other(L10n.tr("error.sub2api.missing_accounts"))
            }
            return page
        } catch let error as HTTPFailure {
            throw error
        } catch {
            throw HTTPFailure.other(error.localizedDescription)
        }
    }

    private static func requestQuota(
        baseURL: URL,
        accountID: Int64,
        token: String,
        session: URLSession
    ) async throws -> UsageAPIResponse {
        let url = baseURL
            .appendingPathComponent("admin", isDirectory: true)
            .appendingPathComponent("openai", isDirectory: true)
            .appendingPathComponent("accounts", isDirectory: true)
            .appendingPathComponent(String(accountID), isDirectory: true)
            .appendingPathComponent("quota", isDirectory: false)
        var request = URLRequest(url: url)
        request.timeoutInterval = RequestPolicy.timeout
        request.httpMethod = "GET"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        let (data, response) = try await perform(request, session: session)
        guard (200..<300).contains(response.statusCode) else {
            throw httpFailure(statusCode: response.statusCode, data: data)
        }

        do {
            let envelope = try JSONDecoder().decode(Sub2APIQuotaEnvelope.self, from: data)
            guard envelope.code == 0 else {
                throw HTTPFailure.other(envelope.message ?? "API code \(envelope.code)")
            }
            guard let payload = envelope.data else {
                throw HTTPFailure.other(L10n.tr("error.sub2api.missing_quota"))
            }
            return payload
        } catch let error as HTTPFailure {
            throw error
        } catch {
            throw HTTPFailure.other(error.localizedDescription)
        }
    }

    private static func perform(
        _ request: URLRequest,
        session: URLSession
    ) async throws -> (Data, HTTPURLResponse) {
        do {
            let (data, response) = try await session.data(for: request)
            guard let response = response as? HTTPURLResponse else {
                throw HTTPFailure.other("Invalid HTTP response")
            }
            return (data, response)
        } catch let failure as HTTPFailure {
            throw failure
        } catch {
            throw HTTPFailure.other(error.localizedDescription)
        }
    }

    private static func httpFailure(statusCode: Int, data: Data) -> HTTPFailure {
        if statusCode == 401 || statusCode == 403 {
            return .unauthorized(
                APIResponseMessage.message(from: data) ?? "HTTP \(statusCode)"
            )
        }
        let message = APIResponseMessage.message(from: data) ?? "HTTP \(statusCode)"
        return .other(message)
    }

    private static func loginToken(from data: Data) -> String? {
        guard let root = try? JSONSerialization.jsonObject(with: data) else { return nil }
        return findToken(in: root)
    }

    private static func findToken(in value: Any) -> String? {
        if let dictionary = value as? [String: Any] {
            for key in ["access_token", "accessToken", "auth_token", "admin_token", "token"] {
                if let token = dictionary[key] as? String,
                   !token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    return token
                }
            }
            for nested in dictionary.values {
                if let token = findToken(in: nested) { return token }
            }
        } else if let array = value as? [Any] {
            for nested in array {
                if let token = findToken(in: nested) { return token }
            }
        }
        return nil
    }

    private static func resolveAdminBaseURL(
        configuredValue: String,
        providerBaseURL: String?
    ) throws -> URL {
        if !configuredValue.isEmpty {
            guard var components = URLComponents(string: configuredValue),
                  Self.isSupportedScheme(components.scheme),
                  components.host != nil else {
                throw AppError.invalidData(L10n.tr("error.sub2api.invalid_base_url"))
            }
            let path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            if path.isEmpty {
                components.path = "/api/v1"
            } else if !path.hasSuffix("api/v1") {
                components.path = "/\(path)/api/v1"
            }
            components.query = nil
            components.fragment = nil
            guard let url = components.url else {
                throw AppError.invalidData(L10n.tr("error.sub2api.invalid_base_url"))
            }
            return url
        }

        guard let providerBaseURL,
              var components = URLComponents(string: providerBaseURL),
              Self.isSupportedScheme(components.scheme),
              components.host != nil else {
            throw AppError.invalidData(L10n.tr("error.sub2api.invalid_base_url"))
        }
        components.path = "/api/v1"
        components.query = nil
        components.fragment = nil
        guard let url = components.url else {
            throw AppError.invalidData(L10n.tr("error.sub2api.invalid_base_url"))
        }
        return url
    }

    private static func isSupportedScheme(_ scheme: String?) -> Bool {
        guard let scheme else { return false }
        return scheme.caseInsensitiveCompare("http") == .orderedSame
            || scheme.caseInsensitiveCompare("https") == .orderedSame
    }
}

private enum APIResponseMessage {
    static func code(from data: Data) -> Int? {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let number = root["code"] as? NSNumber else {
            return nil
        }
        return number.intValue
    }

    static func message(from data: Data) -> String? {
        guard let root = try? JSONSerialization.jsonObject(with: data) else {
            return String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return findMessage(in: root)
    }

    private static func findMessage(in value: Any) -> String? {
        if let dictionary = value as? [String: Any] {
            for key in ["message", "detail", "error"] {
                if let message = dictionary[key] as? String, !message.isEmpty {
                    return message
                }
                if let nested = dictionary[key], let message = findMessage(in: nested) {
                    return message
                }
            }
        }
        return nil
    }
}

private struct ResolvedUsagePayload: Sendable {
    let endpoint: String
    let payload: UsageAPIResponse
}

fileprivate struct UsageAPIResponse: Decodable {
    var accountID: String?
    var email: String?
    var planType: String?
    var rateLimit: RateLimitDetails?
    var additionalRateLimits: [AdditionalRateLimitDetails]?
    var credits: CreditDetails?

    enum CodingKeys: String, CodingKey {
        case accountID = "account_id"
        case email
        case planType = "plan_type"
        case rateLimit = "rate_limit"
        case additionalRateLimits = "additional_rate_limits"
        case credits
    }
}

private struct Sub2APIUsageResult: Sendable {
    var usage: UsageSnapshot
    var email: String?
    var accountID: String?
}

private struct Sub2APIAccountListEnvelope: Decodable {
    var code: Int
    var message: String?
    var data: Sub2APIAccountListPage?
}

private struct Sub2APIAccountListPage: Decodable {
    var items: [Sub2APIRemoteAccount]
    var pages: Int
}

private struct Sub2APIRemoteAccount: Decodable, Sendable {
    var id: Int64
    var name: String
    var platform: String
    var type: String
    var status: String
    var errorMessage: String?
    var parentAccountID: Int64?

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case platform
        case type
        case status
        case errorMessage = "error_message"
        case parentAccountID = "parent_account_id"
    }

    func summary(
        usage: UsageSnapshot?,
        email: String?,
        accountID: String?,
        usageError: String?,
        providerID: String
    ) -> Sub2APIAccountSummary {
        Sub2APIAccountSummary(
            id: id,
            name: name,
            email: email ?? (name.contains("@") ? name : nil),
            accountID: accountID,
            accountType: type,
            status: status,
            planType: usage?.planType,
            usage: usage,
            usageError: usageError ?? errorMessage,
            providerID: providerID
        )
    }
}

private struct Sub2APIQuotaEnvelope: Decodable {
    var code: Int
    var message: String?
    var data: UsageAPIResponse?
}

private struct RateLimitDetails: Decodable {
    var primaryWindow: UsageWindowRaw?
    var secondaryWindow: UsageWindowRaw?

    enum CodingKeys: String, CodingKey {
        case primaryWindow = "primary_window"
        case secondaryWindow = "secondary_window"
    }
}

private struct AdditionalRateLimitDetails: Decodable {
    var rateLimit: RateLimitDetails?

    enum CodingKeys: String, CodingKey {
        case rateLimit = "rate_limit"
    }
}

struct UsageWindowRaw: Equatable {
    var usedPercent: Double
    var limitWindowSeconds: Int64
    var resetAt: Int64
}

extension UsageWindowRaw: Decodable {
    enum CodingKeys: String, CodingKey {
        case usedPercent = "used_percent"
        case limitWindowSeconds = "limit_window_seconds"
        case resetAt = "reset_at"
    }
}

private struct CreditDetails: Decodable {
    var hasCredits: Bool
    var unlimited: Bool
    var balance: String?

    enum CodingKeys: String, CodingKey {
        case hasCredits = "has_credits"
        case unlimited
        case balance
    }
}

private extension String {
    func removingSuffix(_ suffix: String) -> String? {
        guard hasSuffix(suffix) else { return nil }
        return String(dropLast(suffix.count))
    }

    var nonEmptyTrimmed: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

#if DEBUG
extension DefaultUsageService {
    static func debugRequestLogSummary(for request: URLRequest) -> String {
        requestLogSummary(for: request)
    }

    static func debugResponseLogBody(for data: Data) -> String {
        responseLogBody(for: data)
    }
}
#endif
