import XCTest
@testable import Copool

final class ResetCreditsTests: XCTestCase {
    func testDedicatedCountAndHeadersTakePrecedence() async throws {
        let usage = try await fetch(dedicated: #"{"available_count":3}"#, embedded: #"{"available_count":9}"#)
        XCTAssertEqual(usage.remainingResetCount, 3)
        let account = AccountSummary(
            id: "test", label: "test", email: nil, accountID: "account-1",
            planType: nil, teamName: nil, teamAlias: nil, addedAt: 0, updatedAt: 0,
            usage: usage, usageError: nil, isCurrent: false
        )
        XCTAssertEqual(AccountCardPresentation(
            account: account, isCollapsed: false, locale: Locale(identifier: "en_US"),
            usageProgressDisplayMode: .used
        ).remainingResetCountText, "3")
        XCTAssertEqual(try JSONDecoder().decode(UsageSnapshot.self, from: JSONEncoder().encode(usage)), usage)
    }

    func testUnavailableEndpointFallsBackToEmbeddedCount() async throws {
        let usage = try await fetch(dedicated: "{}", embedded: #"{"available_count":"2"}"#, status: 404)
        XCTAssertEqual(usage.remainingResetCount, 2)
    }

    func testZeroAndMalformedCounts() async throws {
        for (body, expected) in [
            (#"{"available_count":0}"#, Optional(0)),
            (#"{"available_count":-1}"#, nil),
            (#"{"available_count":true}"#, nil),
            (#"{"available_count":"invalid"}"#, nil),
            ("invalid json", nil)
        ] {
            let usage = try await fetch(dedicated: body)
            XCTAssertEqual(usage.remainingResetCount, expected)
            XCTAssertEqual(usage.planType, "plus")
        }
    }

    func testOlderSnapshotDecodesWithoutResetCount() throws {
        let usage = try JSONDecoder().decode(UsageSnapshot.self, from: Data(#"{"fetchedAt":1}"#.utf8))
        XCTAssertNil(usage.remainingResetCount)
    }

    private func fetch(dedicated: String, embedded: String = "null", status: Int = 200) async throws -> UsageSnapshot {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [ResetCreditsMockURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        await ResetCreditsMockURLProtocol.store.setHandler { request in
            let url = try XCTUnwrap(request.url)
            let isReset = url.path.hasSuffix("/rate-limit-reset-credits")
            if isReset {
                XCTAssertEqual(request.httpMethod, "GET")
                XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-token")
                XCTAssertEqual(request.value(forHTTPHeaderField: "ChatGPT-Account-Id"), "account-1")
                XCTAssertEqual(request.value(forHTTPHeaderField: "OpenAI-Beta"), "codex-1")
                XCTAssertEqual(request.value(forHTTPHeaderField: "originator"), "Codex Desktop")
            }
            let body = isReset ? dedicated : "{\"plan_type\":\"plus\",\"rate_limit_reset_credits\":\(embedded)}"
            return (HTTPURLResponse(url: url, statusCode: isReset ? status : 200, httpVersion: nil, headerFields: nil)!, Data(body.utf8))
        }
        return try await DefaultUsageService(session: session, configPath: URL(fileURLWithPath: "/tmp/nonexistent-reset-config.toml"))
            .fetchUsage(accessToken: "test-token", accountID: "account-1")
    }
}

private final class ResetCreditsMockURLProtocol: URLProtocol, @unchecked Sendable {
    static let store = ResetCreditsMockURLProtocolStore()

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

private actor ResetCreditsMockURLProtocolStore {
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
