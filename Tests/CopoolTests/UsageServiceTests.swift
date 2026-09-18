import XCTest
@testable import Copool

final class UsageServiceTests: XCTestCase {
    override func tearDown() {
        super.tearDown()
        let resetExpectation = expectation(description: "reset usage mock url protocol")
        Task {
            await UsageMockURLProtocol.store.reset()
            resetExpectation.fulfill()
        }
        wait(for: [resetExpectation], timeout: 1)
    }

    func testFetchUsageShowsDirectServerErrorMessageForExpiredToken() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UsageMockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        let service = DefaultUsageService(
            session: session,
            configPath: URL(fileURLWithPath: "/tmp/nonexistent-config.toml")
        )

        await UsageMockURLProtocol.store.setHandler { request in
            let url = try XCTUnwrap(request.url?.absoluteString)
            if url.contains("/backend-api/wham/usage") {
                return (
                    HTTPURLResponse(
                        url: try XCTUnwrap(request.url),
                        statusCode: 401,
                        httpVersion: nil,
                        headerFields: nil
                    )!,
                    Data(#"{"error":{"message":"Provided authentication token is expired. Please try signing in again.","code":"token_expired"}}"#.utf8)
                )
            }

            return (
                HTTPURLResponse(
                    url: try XCTUnwrap(request.url),
                    statusCode: 403,
                    httpVersion: nil,
                    headerFields: nil
                )!,
                Data("<html>forbidden</html>".utf8)
            )
        }

        do {
            _ = try await service.fetchUsage(accessToken: "expired-token", accountID: "account-1")
            XCTFail("Expected usage request to fail")
        } catch {
            XCTAssertEqual(
                error.localizedDescription,
                "Provided authentication token is expired. Please try signing in again."
            )
        }
    }

    func testCodexModelProviderResolverUsesActiveProfileProviderAndBaseURL() {
        let provider = CodexModelProviderResolver.resolve(raw: """
        model_provider = "openai"
        profile = "work"

        [profiles.work]
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test:6060/v1/"
        """)

        XCTAssertEqual(
            provider,
            CodexModelProviderDefinition(
                id: "my",
                baseURL: "https://sub2.test:6060/v1"
            )
        )
        XCTAssertFalse(provider.isOfficialOpenAI)
    }

    func testCodexModelProviderResolverReadsProxyRoutingMetadata() {
        let provider = CodexModelProviderResolver.resolve(raw: """
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test/v1"
        wire_api = "responses"
        env_key = "MY_SUB2API_API_KEY"
        requires_openai_auth = false
        """)

        XCTAssertEqual(provider.id, "my")
        XCTAssertEqual(provider.baseURL, "https://sub2.test/v1")
        XCTAssertEqual(provider.wireAPI, "responses")
        XCTAssertEqual(provider.envKey, "MY_SUB2API_API_KEY")
        XCTAssertEqual(provider.requiresOpenAIAuth, false)
    }

    func testCodexModelProviderDefinitionsIncludeSiblingProfileConfigs() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let configPath = directory.appendingPathComponent("config.toml")
        try "model_provider = \"openai\"\n".write(
            to: configPath,
            atomically: true,
            encoding: .utf8
        )
        try """
        model_provider = "profile-sub2api"

        [model_providers.profile-sub2api]
        base_url = "https://profile-sub2.test/v1/"
        wire_api = "responses"
        env_key = "PROFILE_SUB2API_KEY"
        requires_openai_auth = false
        """.write(
            to: directory.appendingPathComponent("work.config.toml"),
            atomically: true,
            encoding: .utf8
        )

        let definitions = CodexModelProviderResolver.definitions(configPath: configPath)
        let provider = try XCTUnwrap(definitions.first {
            $0.id.caseInsensitiveCompare("profile-sub2api") == .orderedSame
        })

        XCTAssertEqual(provider.baseURL, "https://profile-sub2.test/v1")
        XCTAssertEqual(provider.wireAPI, "responses")
        XCTAssertEqual(provider.envKey, "PROFILE_SUB2API_KEY")
        XCTAssertEqual(provider.requiresOpenAIAuth, false)
        XCTAssertEqual(CodexModelProviderResolver.resolve(configPath: configPath).id, "openai")
    }

    func testCodexModelProviderDefinitionsIncludeProfileRootProviderWithoutDefinition() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let configPath = directory.appendingPathComponent("config.toml")
        try "model_provider = \"openai\"\n".write(
            to: configPath,
            atomically: true,
            encoding: .utf8
        )
        try "model_provider = \"profile-sub2api\"\n".write(
            to: directory.appendingPathComponent("work.config.toml"),
            atomically: true,
            encoding: .utf8
        )

        let definitions = CodexModelProviderResolver.definitions(configPath: configPath)

        XCTAssertTrue(definitions.contains {
            $0.id.caseInsensitiveCompare("profile-sub2api") == .orderedSame
        })
    }

    func testCodexProviderResolverMatchesLegacyAdminURLForMigration() throws {
        let configPath = try makeCodexConfig("""
        model_provider = "my"

        [model_providers.my]
        base_url = "https://local-sub2.test:6060"

        [model_providers.ShareCoder]
        base_url = "https://sharecoder.test"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        XCTAssertEqual(
            CodexModelProviderResolver.providerID(
                matchingBaseURL: "https://sharecoder.test/api/v1",
                configPath: configPath
            ),
            "ShareCoder"
        )
    }

    func testCodexModelProviderSwitchServiceUpdatesRootProviderAndPreservesTables() throws {
        let configPath = try makeCodexConfig("""
        model_provider = "openai" # selected provider

        [model_providers.ShareCoder]
        base_url = "https://sub2.test/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        try CodexModelProviderSwitchService(configPath: configPath).switchProvider(to: "ShareCoder")

        let raw = try String(contentsOf: configPath, encoding: .utf8)
        XCTAssertTrue(raw.contains("model_provider = \"ShareCoder\" # selected provider"))
        XCTAssertTrue(raw.contains("[model_providers.ShareCoder]"))
        XCTAssertEqual(CodexModelProviderResolver.resolve(raw: raw).id, "ShareCoder")
    }

    func testCodexModelProviderSwitchServiceUpdatesActiveProfileOverride() throws {
        let configPath = try makeCodexConfig("""
        model_provider = "openai"
        profile = "work"

        [profiles.work]
        model_provider = "my"

        [model_providers.ShareCoder]
        base_url = "https://sub2.test/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        try CodexModelProviderSwitchService(configPath: configPath).switchProvider(to: "ShareCoder")

        let raw = try String(contentsOf: configPath, encoding: .utf8)
        XCTAssertTrue(raw.contains("model_provider = \"openai\""))
        XCTAssertTrue(raw.contains("[profiles.work]\nmodel_provider = \"ShareCoder\""))
        XCTAssertEqual(CodexModelProviderResolver.resolve(raw: raw).id, "ShareCoder")
    }

    func testCodexModelProviderSwitchServiceAddsRootProviderBeforeFirstTable() throws {
        let configPath = try makeCodexConfig("""
        # Keep provider definitions intact.
        [model_providers.ShareCoder]
        base_url = "https://sub2.test/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        try CodexModelProviderSwitchService(configPath: configPath).switchProvider(to: "ShareCoder")

        let raw = try String(contentsOf: configPath, encoding: .utf8)
        XCTAssertTrue(raw.contains("model_provider = \"ShareCoder\"\n[model_providers.ShareCoder]"))
        XCTAssertEqual(CodexModelProviderResolver.resolve(raw: raw).id, "ShareCoder")
    }

    func testCustomDefaultProviderListsAccountsAndMapsSub2APIQuota() async throws {
        let configPath = try makeCodexConfig("""
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test:6060/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        let providerConfiguration = Sub2APIProviderConfiguration(
            providerID: "my",
            username: "you@example.com"
        )
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my"],
            providers: [providerConfiguration]
        )
        let secretStore = UsageStubSub2APISecretStore(
            passwords: [providerConfiguration.id: "secret"]
        )
        let recorder = UsageProviderRequestRecorder()
        await UsageMockURLProtocol.store.setHandler { request in
            recorder.record(request)
            let url = try XCTUnwrap(request.url)
            switch url.path {
            case "/api/v1/auth/login":
                return (
                    HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil)!,
                    Data(#"{"code":0,"message":"success","data":{"access_token":"admin-token"}}"#.utf8)
                )
            case "/api/v1/admin/accounts":
                let page = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                    .queryItems?.first(where: { $0.name == "page" })?.value
                let body: String
                if page == "2" {
                    body = #"{"code":0,"message":"success","data":{"items":[{"id":2,"name":"spark-shadow","platform":"openai","type":"oauth","status":"active","error_message":null,"parent_account_id":1},{"id":3,"name":"paused@example.com","platform":"openai","type":"oauth","status":"inactive","error_message":null,"parent_account_id":null}],"total":3,"page":2,"page_size":200,"pages":2}}"#
                } else {
                    body = #"{"code":0,"message":"success","data":{"items":[{"id":1,"name":"openai-2026","platform":"openai","type":"oauth","status":"active","error_message":null,"parent_account_id":null}],"total":3,"page":1,"page_size":200,"pages":2}}"#
                }
                return (
                    HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil)!,
                    Data(body.utf8)
                )
            case "/api/v1/admin/openai/accounts/1/quota":
                return (
                    HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil)!,
                    Data(#"{"code":0,"message":"success","data":{"email":"you@example.com","plan_type":"prolite","rate_limit":{"primary_window":{"used_percent":38,"limit_window_seconds":604800,"reset_at":1787810131}},"additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_at":1787449578}}}]}}"#.utf8)
                )
            default:
                XCTFail("Unexpected URL: \(url.absoluteString)")
                return (
                    HTTPURLResponse(url: url, statusCode: 404, httpVersion: nil, headerFields: nil)!,
                    Data()
                )
            }
        }

        let session = makeUsageMockSession()
        let service = DefaultSub2APIAccountService(
            configPath: configPath,
            settingsRepository: StaticUsageSettingsRepository(settings: settings),
            secretStore: secretStore,
            session: session,
            insecureSession: session,
            dateProvider: UsageFixedDateProvider(now: 1_787_431_578)
        )

        XCTAssertTrue(service.isConnectionConfigured())
        XCTAssertTrue(service.canQueryCurrentDefaultProvider())
        let accounts = try await service.fetchAccounts(accountIDs: nil)
        XCTAssertEqual(accounts.count, 1)
        let account = try XCTUnwrap(accounts.first)
        let usage = try XCTUnwrap(account.usage)
        XCTAssertEqual(account.displayEmail, "you@example.com")
        XCTAssertEqual(account.accountSummary.normalizedPlanLabel, "TEAM")
        XCTAssertNil(account.accountSummary.displayTeamName)

        XCTAssertEqual(usage.fetchedAt, 1_787_431_578)
        XCTAssertEqual(usage.planType, "prolite")
        XCTAssertEqual(usage.fiveHour?.usedPercent, 0)
        XCTAssertEqual(usage.fiveHour?.windowSeconds, 18_000)
        XCTAssertEqual(usage.fiveHour?.resetAt, 1_787_449_578)
        XCTAssertEqual(usage.oneWeek?.usedPercent, 38)
        XCTAssertEqual(usage.oneWeek?.windowSeconds, 604_800)
        XCTAssertEqual(usage.oneWeek?.resetAt, 1_787_810_131)

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map { $0.url.path }, [
            "/api/v1/auth/login",
            "/api/v1/admin/accounts",
            "/api/v1/admin/accounts",
            "/api/v1/admin/openai/accounts/1/quota"
        ])
        let listQueryItems = URLComponents(url: requests[1].url, resolvingAgainstBaseURL: false)?.queryItems
        XCTAssertEqual(listQueryItems?.first(where: { $0.name == "platform" })?.value, "openai")
        XCTAssertEqual(listQueryItems?.first(where: { $0.name == "type" })?.value, "oauth")
        XCTAssertEqual(listQueryItems?.first(where: { $0.name == "status" })?.value, "active")
        let loginBody = try XCTUnwrap(requests.first?.body)
        XCTAssertEqual(
            try JSONSerialization.jsonObject(with: loginBody) as? NSDictionary,
            ["email": "you@example.com", "password": "secret"] as NSDictionary
        )
        XCTAssertEqual(requests.last?.authorization, "Bearer admin-token")
    }

    func testConfiguredSub2APIIsNotQueriedWhenItIsNotTheDefaultProvider() async throws {
        let configPath = try makeCodexConfig("""
        model_provider = "other"

        [model_providers.my]
        base_url = "https://sub2.test:6060/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "you@example.com",
                    password: "secret"
                )
            ]
        )
        let recorder = UsageProviderRequestRecorder()
        await UsageMockURLProtocol.store.setHandler { request in
            recorder.record(request)
            let url = try XCTUnwrap(request.url)
            return (
                HTTPURLResponse(url: url, statusCode: 500, httpVersion: nil, headerFields: nil)!,
                Data()
            )
        }

        let session = makeUsageMockSession()
        let service = DefaultSub2APIAccountService(
            configPath: configPath,
            settingsRepository: StaticUsageSettingsRepository(settings: settings),
            session: session,
            insecureSession: session
        )

        XCTAssertFalse(service.isConnectionConfigured())
        XCTAssertFalse(service.canQueryCurrentDefaultProvider())
        do {
            _ = try await service.fetchAccounts(accountIDs: nil)
            XCTFail("Expected provider mismatch")
        } catch {
            XCTAssertEqual(error.localizedDescription, L10n.tr("error.sub2api.provider_not_confirmed"))
        }

        let requests = recorder.snapshot()
        XCTAssertTrue(requests.isEmpty)
    }

    func testMultipleSub2APIConfigurationsUseCurrentProviderCredentialsAndBaseURL() async throws {
        let configPath = try makeCodexConfig("""
        model_provider = "ShareCoder"

        [model_providers.my]
        base_url = "https://my-sub2.test:6060/v1"

        [model_providers.ShareCoder]
        base_url = "https://sharecoder.test/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my", "ShareCoder"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "my-admin@example.com",
                    password: "my-secret"
                ),
                Sub2APIProviderConfiguration(
                    providerID: "ShareCoder",
                    username: "share-admin@example.com",
                    password: "share-secret"
                ),
            ]
        )
        let recorder = UsageProviderRequestRecorder()
        await UsageMockURLProtocol.store.setHandler { request in
            recorder.record(request)
            let url = try XCTUnwrap(request.url)
            switch url.path {
            case "/api/v1/auth/login":
                return (
                    HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil)!,
                    Data(#"{"code":0,"data":{"access_token":"share-token"}}"#.utf8)
                )
            case "/api/v1/admin/accounts":
                return (
                    HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: nil)!,
                    Data(#"{"code":0,"data":{"items":[],"total":0,"page":1,"page_size":200,"pages":1}}"#.utf8)
                )
            default:
                XCTFail("Unexpected URL: \(url.absoluteString)")
                return (
                    HTTPURLResponse(url: url, statusCode: 404, httpVersion: nil, headerFields: nil)!,
                    Data()
                )
            }
        }
        let session = makeUsageMockSession()
        let service = DefaultSub2APIAccountService(
            configPath: configPath,
            settingsRepository: StaticUsageSettingsRepository(settings: settings),
            session: session,
            insecureSession: session
        )

        let accounts = try await service.fetchAccounts(accountIDs: nil)
        XCTAssertEqual(accounts, [])

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.first?.url.host, "sharecoder.test")
        let loginBody = try XCTUnwrap(requests.first?.body)
        XCTAssertEqual(
            try JSONSerialization.jsonObject(with: loginBody) as? NSDictionary,
            ["email": "share-admin@example.com", "password": "share-secret"] as NSDictionary
        )
    }

    func testConfiguredProviderDoesNotRequireExplicitConfirmation() throws {
        let configPath = try makeCodexConfig("""
        model_provider = "my"

        [model_providers.my]
        base_url = "https://sub2.test:6060/v1"
        """)
        defer { try? FileManager.default.removeItem(at: configPath) }

        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: "my",
                    username: "you@example.com",
                    password: "secret"
                )
            ]
        )
        let session = makeUsageMockSession()
        let service = DefaultSub2APIAccountService(
            configPath: configPath,
            settingsRepository: StaticUsageSettingsRepository(settings: settings),
            session: session,
            insecureSession: session
        )

        XCTAssertEqual(service.currentDefaultProviderID(), "my")
        XCTAssertTrue(service.isConnectionConfigured())
        XCTAssertTrue(service.canQueryCurrentDefaultProvider())
    }

    func testBackgroundNetworkSessionDisablesPersistentHTTPStorage() {
        let configuration = BackgroundNetworkSession.shared.configuration

        XCTAssertEqual(configuration.identifier, nil)
        XCTAssertEqual(configuration.requestCachePolicy, .reloadIgnoringLocalCacheData)
        XCTAssertNil(configuration.urlCache)
        XCTAssertNil(configuration.httpCookieStorage)
        XCTAssertFalse(configuration.httpShouldSetCookies)
    }

    private func makeCodexConfig(_ contents: String) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("copool-codex-config-\(UUID().uuidString).toml")
        try contents.write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    private func makeUsageMockSession() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [UsageMockURLProtocol.self]
        return URLSession(configuration: configuration)
    }

    func testUsageDebugRequestSummaryIncludesRequestDetails() throws {
        var request = URLRequest(url: try XCTUnwrap(URL(string: "https://chatgpt.com/backend-api/wham/usage")))
        request.httpMethod = "GET"
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("account-1", forHTTPHeaderField: "ChatGPT-Account-Id")
        request.setValue("codex-tools-swift/0.1", forHTTPHeaderField: "User-Agent")

        let summary = DefaultUsageService.debugRequestLogSummary(for: request)
        let payload = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: Data(summary.utf8)) as? NSDictionary
        )

        XCTAssertEqual(
            payload,
            [
                "method": "GET",
                "url": "https://chatgpt.com/backend-api/wham/usage",
                "headers": [
                    "Accept": "application/json",
                    "ChatGPT-Account-Id": "account-1",
                    "User-Agent": "codex-tools-swift/0.1"
                ]
            ] as NSDictionary
        )
    }

    func testUsageDebugResponseBodyReturnsRawJSONPayload() {
        let body = DefaultUsageService.debugResponseLogBody(
            for: Data(#"{"detail":{"code":"deactivated_workspace"}}"#.utf8)
        )

        XCTAssertEqual(body, #"{"detail":{"code":"deactivated_workspace"}}"#)
    }
}

private struct StaticUsageSettingsRepository: SettingsRepository {
    let settings: AppSettings

    func loadSettings() throws -> AppSettings {
        settings
    }

    func saveSettings(_ settings: AppSettings) throws {
        _ = settings
    }
}

private struct UsageStubSub2APISecretStore: Sub2APISecretStoreProtocol {
    let passwords: [UUID: String]

    func password(for configurationID: UUID) throws -> String? {
        passwords[configurationID]
    }

    func setPassword(_ password: String, for configurationID: UUID) throws {
        _ = password
        _ = configurationID
    }

    func removePassword(for configurationID: UUID) throws {
        _ = configurationID
    }
}

private struct UsageFixedDateProvider: DateProviding {
    let now: Int64

    func unixSecondsNow() -> Int64 {
        now
    }
}

private final class UsageProviderRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [UsageProviderRecordedRequest] = []

    func record(_ request: URLRequest) {
        guard let url = request.url else { return }
        let body = request.httpBody ?? request.httpBodyStream.flatMap(Self.readBody)
        let recorded = UsageProviderRecordedRequest(
            url: url,
            authorization: request.value(forHTTPHeaderField: "Authorization"),
            body: body
        )
        lock.lock()
        requests.append(recorded)
        lock.unlock()
    }

    func snapshot() -> [UsageProviderRecordedRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }

    private static func readBody(from stream: InputStream) -> Data? {
        stream.open()
        defer { stream.close() }

        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 1_024)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count < 0 { return nil }
            if count == 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

private struct UsageProviderRecordedRequest {
    var url: URL
    var authorization: String?
    var body: Data?
}

private final class UsageMockURLProtocol: URLProtocol, @unchecked Sendable {
    static let store = UsageMockURLProtocolStore()

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        Task {
            do {
                guard let handler = await Self.store.handler() else {
                    client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
                    return
                }

                let (response, data) = try handler(request)
                client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
                client?.urlProtocol(self, didLoad: data)
                client?.urlProtocolDidFinishLoading(self)
            } catch {
                client?.urlProtocol(self, didFailWithError: error)
            }
        }
    }

    override func stopLoading() {}
}

private actor UsageMockURLProtocolStore {
    typealias Handler = @Sendable (URLRequest) throws -> (HTTPURLResponse, Data)

    private var currentHandler: Handler?

    func setHandler(_ handler: @escaping Handler) {
        currentHandler = handler
    }

    func handler() -> Handler? {
        currentHandler
    }

    func reset() {
        currentHandler = nil
    }
}
