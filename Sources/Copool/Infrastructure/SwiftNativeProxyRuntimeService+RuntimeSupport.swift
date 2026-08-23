import Foundation

extension SwiftNativeProxyRuntimeService {
    func makeUpstreamRequest(
        payload: [String: Any],
        candidate: ProxyCandidate,
        downstreamHeaders: [String: String]
    ) throws -> URLRequest {
        let body = try JSONSerialization.data(withJSONObject: payload)
        let upstreamModel = (payload["model"] as? String) ?? "gpt-5.4"
        let userAgent = Self.normalizedForwardHeader(downstreamHeaders["user-agent"])
            ?? Self.defaultCodexUserAgent
        let version = Self.normalizedForwardHeader(downstreamHeaders["version"])
            ?? Self.parseCodexVersion(fromUserAgent: userAgent)
            ?? Self.defaultCodexClientVersion
        let sessionID = Self.normalizedForwardHeader(downstreamHeaders["session_id"])
            ?? Self.normalizedForwardHeader(downstreamHeaders["session-id"])
            ?? UUID().uuidString
        let endpoint: URL
        switch candidate.route {
        case .chatGPTOAuth:
            endpoint = responsesEndpoint(forUpstreamModel: upstreamModel)
        case .modelProvider(_, let baseURL):
            endpoint = try modelProviderResponsesEndpoint(configuredBaseURL: baseURL)
        }
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.timeoutInterval = 180
        request.httpBody = body
        request.setValue("Bearer \(candidate.accessToken)", forHTTPHeaderField: "Authorization")
        if candidate.route == .chatGPTOAuth {
            request.setValue(candidate.accountID, forHTTPHeaderField: "ChatGPT-Account-Id")
        }
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("codex_cli_rs", forHTTPHeaderField: "Originator")
        request.setValue(version, forHTTPHeaderField: "Version")
        request.setValue(sessionID, forHTTPHeaderField: "Session_id")
        request.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        request.setValue("Keep-Alive", forHTTPHeaderField: "Connection")
        return request
    }

    func currentCandidates() throws -> [ProxyCandidate] {
        let sourceDates = candidateSourceDates()
        if let cachedCandidates,
           cachedCandidateSourceDates == sourceDates {
            return applyRuntimeCandidateOrdering(to: cachedCandidates)
        }

        let candidates = try loadCandidates()
        cachedCandidates = candidates
        cachedCandidateSourceDates = sourceDates
        return applyRuntimeCandidateOrdering(to: candidates)
    }

    func loadCandidates() throws -> [ProxyCandidate] {
        let store = try storeRepository.loadStore()
        let currentAccountID = store.currentAccountID
        let currentProvider = CodexModelProviderResolver.resolve(configPath: paths.codexConfigPath)
        let prefersLocalAccount = currentProvider.isOfficialOpenAI

        let localCandidates = try store.accounts.compactMap { account -> ProxyCandidate? in
            let extracted = try authRepository.extractAuth(from: account.authJSON)
            return ProxyCandidate(
                id: account.id,
                label: account.label,
                accountID: extracted.accountID,
                accountKey: account.accountKey,
                accessToken: extracted.accessToken,
                authJSON: account.authJSON,
                addedAt: account.addedAt,
                isPreferredCurrent: prefersLocalAccount && account.id == currentAccountID,
                oneWeekUsed: account.usage?.oneWeek?.usedPercent,
                fiveHourUsed: account.usage?.fiveHour?.usedPercent
            )
        }
        let providerCandidates = (try? loadSub2APIProviderCandidates(
            currentProviderID: currentProvider.id
        )) ?? []
        let candidates = localCandidates + providerCandidates

        return candidates.sorted { lhs, rhs in
            if lhs.isPreferredCurrent != rhs.isPreferredCurrent {
                return lhs.isPreferredCurrent
            }
            if lhs.remainingScore != rhs.remainingScore {
                return lhs.remainingScore > rhs.remainingScore
            }
            if lhs.addedAt != rhs.addedAt {
                return lhs.addedAt < rhs.addedAt
            }
            return lhs.id < rhs.id
        }
    }

    func loadSub2APIProviderCandidates(currentProviderID: String) throws -> [ProxyCandidate] {
        let settings = try settingsRepository.loadSettings().sub2APIProvider.normalized()
        let definitions = CodexModelProviderResolver.definitions(configPath: paths.codexConfigPath)

        return settings.providers.compactMap { rawConfiguration in
            let configuration = rawConfiguration.normalized()
            guard !configuration.providerID.isEmpty,
                  !configuration.importedAccountIDs.isEmpty,
                  let provider = definitions.first(where: {
                      $0.id.caseInsensitiveCompare(configuration.providerID) == .orderedSame
                  }),
                  provider.requiresOpenAIAuth != true,
                  (provider.wireAPI ?? "responses")
                      .trimmingCharacters(in: .whitespacesAndNewlines)
                      .caseInsensitiveCompare("responses") == .orderedSame,
                  let rawBaseURL = provider.baseURL?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !rawBaseURL.isEmpty,
                  let rawEnvKey = provider.envKey?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !rawEnvKey.isEmpty,
                  let apiKey = providerAPIKey(environmentKey: rawEnvKey),
                  !apiKey.isEmpty else {
                return nil
            }

            let importedIDs = Set(configuration.importedAccountIDs)
            let importedAccounts = configuration.cachedAccounts.filter { importedIDs.contains($0.id) }
            let representative = importedAccounts.max {
                Self.sub2APIAccountRemainingScore($0) < Self.sub2APIAccountRemainingScore($1)
            }
            let providerID = provider.id
            let routeID = "sub2api-provider:\(providerID.lowercased())"

            return ProxyCandidate(
                id: routeID,
                label: providerID,
                accountID: routeID,
                accountKey: routeID,
                accessToken: apiKey,
                authJSON: .null,
                addedAt: representative?.id ?? configuration.importedAccountIDs.min() ?? 0,
                isPreferredCurrent: currentProviderID.caseInsensitiveCompare(providerID) == .orderedSame,
                oneWeekUsed: representative?.usage?.oneWeek?.usedPercent,
                fiveHourUsed: representative?.usage?.fiveHour?.usedPercent,
                route: .modelProvider(providerID: providerID, baseURL: rawBaseURL),
                allowInsecureTLS: configuration.allowInsecureTLS
            )
        }
    }

    static func sub2APIAccountRemainingScore(_ account: Sub2APIAccountSummary) -> Double {
        let weekUsed = account.usage?.oneWeek?.usedPercent ?? 100
        let fiveUsed = account.usage?.fiveHour?.usedPercent ?? 100
        return max(0, 100 - weekUsed) * 0.7 + max(0, 100 - fiveUsed) * 0.3
    }

    func providerAPIKey(environmentKey: String) -> String? {
        if let value = environment[environmentKey]?.trimmingCharacters(in: .whitespacesAndNewlines),
           !value.isEmpty {
            return value
        }
        return providerEnvironmentFallback(environmentKey)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func applyRuntimeCandidateOrdering(to candidates: [ProxyCandidate]) -> [ProxyCandidate] {
        let now = currentUnixSeconds()
        cooldownUntilByAccountID = cooldownUntilByAccountID.filter { $0.value > now }

        var available: [ProxyCandidate] = []
        available.reserveCapacity(candidates.count)
        for candidate in candidates {
            if let cooldownUntil = cooldownUntilByAccountID[candidate.accountID],
               cooldownUntil > now {
                continue
            }
            available.append(candidate)
        }

        return available.sorted { lhs, rhs in
            if lhs.isPreferredCurrent != rhs.isPreferredCurrent {
                return lhs.isPreferredCurrent
            }
            let lhsSticky = lhs.accountID == stickyAccountID
            let rhsSticky = rhs.accountID == stickyAccountID
            if lhsSticky != rhsSticky {
                return lhsSticky
            }
            if lhs.remainingScore != rhs.remainingScore {
                return lhs.remainingScore > rhs.remainingScore
            }
            if lhs.addedAt != rhs.addedAt {
                return lhs.addedAt < rhs.addedAt
            }
            return lhs.id < rhs.id
        }
    }

    func candidateSourceDates() -> ProxyCandidateSourceDates {
        ProxyCandidateSourceDates(
            accounts: modificationDate(for: paths.accountStorePath),
            settings: modificationDate(for: paths.settingsStorePath),
            codexConfig: modificationDate(for: paths.codexConfigPath)
        )
    }

    func modificationDate(for path: URL) -> Date? {
        let attributes = try? FileManager.default.attributesOfItem(atPath: path.path)
        return attributes?[.modificationDate] as? Date
    }

    func isAuthorized(_ headers: [String: String]) -> Bool {
        guard let expected = try? ensurePersistedAPIKey() else { return false }
        if let apiKeyHeader = headers["x-api-key"]?.trimmingCharacters(in: .whitespacesAndNewlines),
           !apiKeyHeader.isEmpty,
           apiKeyHeader == expected {
            return true
        }

        guard let authorization = headers["authorization"]?.trimmingCharacters(in: .whitespacesAndNewlines),
              !authorization.isEmpty else {
            return false
        }

        let lower = authorization.lowercased()
        if lower.hasPrefix("bearer ") {
            let provided = String(authorization.dropFirst("Bearer ".count)).trimmingCharacters(in: .whitespacesAndNewlines)
            return provided == expected
        }

        return authorization == expected
    }

    func ensurePersistedAPIKey() throws -> String {
        if let key = try readPersistedAPIKey(), !key.isEmpty {
            return key
        }

        let generated = randomAPIKey()
        try persistAPIKey(generated)
        return generated
    }

    func readPersistedAPIKey() throws -> String? {
        guard FileManager.default.fileExists(atPath: paths.proxyDaemonKeyPath.path) else {
            return nil
        }

        let text = try String(contentsOf: paths.proxyDaemonKeyPath, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? nil : text
    }

    func persistAPIKey(_ value: String) throws {
        try FileManager.default.createDirectory(at: paths.proxyDaemonDataDirectory, withIntermediateDirectories: true)
        try value.write(to: paths.proxyDaemonKeyPath, atomically: true, encoding: .utf8)
        #if canImport(Darwin)
        _ = chmod(paths.proxyDaemonKeyPath.path, S_IRUSR | S_IWUSR)
        #endif
    }

    func randomAPIKey() -> String {
        "sk-\(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased())"
    }

    func sendUpstream(
        payload: [String: Any],
        candidate: ProxyCandidate,
        downstreamHeaders: [String: String]
    ) async throws -> UpstreamResponse {
        guard JSONSerialization.isValidJSONObject(payload) else {
            throw AppError.invalidData(L10n.tr("error.proxy_runtime.invalid_upstream_payload"))
        }

        return try await performUpstreamRequest(
            payload: payload,
            candidate: candidate,
            downstreamHeaders: downstreamHeaders
        )
    }

    func performUpstreamRequest(
        payload: [String: Any],
        candidate: ProxyCandidate,
        downstreamHeaders: [String: String]
    ) async throws -> UpstreamResponse {
        let request = try makeUpstreamRequest(
            payload: payload,
            candidate: candidate,
            downstreamHeaders: downstreamHeaders
        )
        let session = candidate.allowInsecureTLS
            ? BackgroundNetworkSession.insecureSub2API
            : URLSession.shared
        let (responseBytes, response) = try await session.bytes(for: request)
        let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 500
        var responseBody = Data()
        responseBody.reserveCapacity(64 * 1024)

        for try await byte in responseBytes {
            responseBody.append(byte)
            if responseBody.count > ProxyRuntimeLimits.maxUpstreamResponseBytes {
                throw AppError.network(
                    L10n.tr(
                        "error.proxy_runtime.upstream_response_too_large_format",
                        ProxyRuntimeLimits.limitDescription(for: ProxyRuntimeLimits.maxUpstreamResponseBytes)
                    )
                )
            }
        }

        return UpstreamResponse(statusCode: statusCode, body: responseBody)
    }

    func openStreamingUpstreamRequest(
        payload: [String: Any],
        candidate: ProxyCandidate,
        downstreamHeaders: [String: String]
    ) async throws -> UpstreamStreamingResponse {
        let request = try makeUpstreamRequest(
            payload: payload,
            candidate: candidate,
            downstreamHeaders: downstreamHeaders
        )
        let session = candidate.allowInsecureTLS
            ? BackgroundNetworkSession.insecureSub2API
            : URLSession.shared
        let (bytes, response) = try await session.bytes(for: request)
        let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 500
        return UpstreamStreamingResponse(statusCode: statusCode, bytes: bytes, candidate: candidate)
    }

    static func shouldSyncCurrentAuthOnSuccessfulProxyResponse(localProxyHostAPIOnly: Bool) -> Bool {
        !localProxyHostAPIOnly
    }

    static func normalizedForwardHeader(_ value: String?) -> String? {
        guard let value else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    static func parseCodexVersion(fromUserAgent value: String?) -> String? {
        guard let userAgent = normalizedForwardHeader(value) else { return nil }
        let prefixes = ["codex_exec/", "codex_cli_rs/"]

        for token in userAgent.split(whereSeparator: { $0.isWhitespace }) {
            let rawToken = String(token)
            guard let prefix = prefixes.first(where: {
                rawToken.lowercased().hasPrefix($0)
            }) else { continue }

            let version = String(rawToken.dropFirst(prefix.count))
            guard isValidCodexVersion(version) else { continue }
            return version
        }
        return nil
    }

    private static func isValidCodexVersion(_ value: String) -> Bool {
        guard !value.isEmpty,
              value.allSatisfy({ $0.isLetter || $0.isNumber || ".-+".contains($0) }) else {
            return false
        }
        let coreEnd = value.firstIndex(where: { $0 == "-" || $0 == "+" }) ?? value.endIndex
        let core = value[..<coreEnd]
        let components = core.split(separator: ".", omittingEmptySubsequences: false)
        return components.count >= 2
            && components.allSatisfy { !$0.isEmpty && $0.allSatisfy(\.isNumber) }
    }

    func shouldSyncCurrentAuthOnSuccessfulProxyResponse() -> Bool {
        let localProxyHostAPIOnly = (try? settingsRepository.loadSettings().localProxyHostAPIOnly)
            ?? AppSettings.defaultValue.localProxyHostAPIOnly
        return Self.shouldSyncCurrentAuthOnSuccessfulProxyResponse(
            localProxyHostAPIOnly: localProxyHostAPIOnly
        )
    }

    static func normalizeConfiguredBaseURL(_ configuredBaseURL: String) -> String {
        var trimmed = configuredBaseURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))

        if trimmed.hasSuffix("/backend-api/codex/responses") {
            trimmed = String(trimmed.dropLast("/responses".count))
        } else if trimmed.hasSuffix("/backend-api/responses") {
            trimmed = String(trimmed.dropLast("/responses".count))
        }

        return trimmed
    }

    func modelProviderResponsesEndpoint(configuredBaseURL: String) throws -> URL {
        var baseURL = configuredBaseURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if baseURL.hasSuffix("/responses") {
            baseURL = String(baseURL.dropLast("/responses".count))
        }
        guard let components = URLComponents(string: baseURL),
              let scheme = components.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              components.host != nil,
              let endpoint = URL(string: "\(baseURL)/responses") else {
            throw AppError.invalidData(L10n.tr("error.sub2api.invalid_base_url"))
        }
        return endpoint
    }

    func readChatGPTBaseURLFromConfig() -> String? {
        guard let raw = try? String(contentsOf: paths.codexConfigPath, encoding: .utf8), !raw.isEmpty else {
            return nil
        }

        for line in raw.split(whereSeparator: { $0.isNewline }) {
            let trimmed = String(line).trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("chatgpt_base_url") else { continue }
            guard let equalIndex = trimmed.firstIndex(of: "=") else { continue }
            let value = trimmed[trimmed.index(after: equalIndex)...]
                .trimmingCharacters(in: CharacterSet.whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            if !value.isEmpty {
                return value
            }
        }

        return nil
    }

    func waitForHealth(port: Int) async -> Bool {
        guard let url = URL(string: "http://127.0.0.1:\(port)/health") else { return false }
        let deadline = Date().addingTimeInterval(6)

        while Date() < deadline {
            do {
                let (_, response) = try await URLSession.shared.data(from: url)
                if let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) {
                    return true
                }
            } catch {
            }
            try? await Task.sleep(for: .milliseconds(250))
        }

        return false
    }
}

struct UpstreamResponse {
    var statusCode: Int
    var body: Data
}

struct UpstreamStreamingResponse {
    var statusCode: Int
    var bytes: URLSession.AsyncBytes
    var candidate: ProxyCandidate
}

struct ProxyCandidate {
    var id: String
    var label: String
    var accountID: String
    var accountKey: String
    var accessToken: String
    var authJSON: JSONValue
    var addedAt: Int64
    var isPreferredCurrent: Bool
    var oneWeekUsed: Double?
    var fiveHourUsed: Double?
    var route: ProxyCandidateRoute = .chatGPTOAuth
    var allowInsecureTLS: Bool = false

    var remainingScore: Double {
        let weekUsed = oneWeekUsed ?? 100
        let fiveUsed = fiveHourUsed ?? 100
        let weekRemaining = max(0, 100 - weekUsed)
        let fiveRemaining = max(0, 100 - fiveUsed)
        return weekRemaining * 0.7 + fiveRemaining * 0.3
    }
}

enum ProxyCandidateRoute: Equatable {
    case chatGPTOAuth
    case modelProvider(providerID: String, baseURL: String)
}

struct ProxyCandidateSourceDates: Equatable {
    var accounts: Date?
    var settings: Date?
    var codexConfig: Date?
}
