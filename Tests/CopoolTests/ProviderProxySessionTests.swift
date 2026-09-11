import XCTest
import Network
@testable import Copool

final class ProviderProxySessionTests: XCTestCase {
    func testPerAccountOverridesSurviveCacheReplacementAndProviderChanges() throws {
        var provider = Sub2APIProviderConfiguration(
            providerID: "p", proxyURL: "http://127.0.0.1:8080",
            accountProxyURLs: ["42": "socks5://127.0.0.1:1080"], importedAccountIDs: [42, 43]
        )
        provider.cachedAccounts = []
        provider.proxyURL = "http://127.0.0.1:8081"
        let decoded = try JSONDecoder().decode(Sub2APIProviderConfiguration.self, from: JSONEncoder().encode(provider))
        XCTAssertEqual(decoded.proxyURL(forAccountID: 42), "socks5://127.0.0.1:1080")
        XCTAssertEqual(decoded.proxyURL(forAccountID: 43), "http://127.0.0.1:8081")
        provider.accountProxyURLs["42"] = nil
        XCTAssertEqual(provider.proxyURL(forAccountID: 42), provider.proxyURL)
    }

    func testLegacyConfigurationAndRoundTrip() throws {
        let legacy = try JSONDecoder().decode(Sub2APIProviderConfiguration.self, from: Data("{}".utf8))
        XCTAssertEqual(legacy.proxyURL, "")
        let provider = Sub2APIProviderConfiguration(providerID: "one", proxyURL: " socks5://127.0.0.1:1080 ").normalized()
        let decoded = try JSONDecoder().decode(Sub2APIProviderConfiguration.self, from: JSONEncoder().encode(provider))
        XCTAssertEqual(decoded.proxyURL, "socks5://127.0.0.1:1080")
    }

    func testRejectsInvalidProxyRatherThanFallingBack() {
        for value in ["localhost:1080", "ftp://localhost:21", "http://localhost", "http://localhost:0",
                      "http://localhost:65536", "http://localhost:80/path", "http://localhost:80?q=1",
                      "http://user:password@localhost:80"] {
            XCTAssertThrowsError(try ProviderProxySession.proxyConfiguration(for: value), value)
        }
    }

    func testRequestsActuallyReachTheirSelectedHTTPProxy() async throws {
        let firstReceived = expectation(description: "First proxy receives CONNECT")
        let secondReceived = expectation(description: "Second proxy receives CONNECT")
        let firstServer = try SimpleHTTPServer(port: 0) { request in
            if request.method == "CONNECT" { firstReceived.fulfill() }
            return HTTPResponse.text(statusCode: 502, text: "test proxy")
        }
        let secondServer = try SimpleHTTPServer(port: 0) { request in
            if request.method == "CONNECT" { secondReceived.fulfill() }
            return HTTPResponse.text(statusCode: 502, text: "test proxy")
        }
        try await firstServer.start()
        try await secondServer.start()
        defer { firstServer.stop(); secondServer.stop() }
        let fallback = URLSession(configuration: .ephemeral)
        defer { fallback.invalidateAndCancel() }
        let pool = ProviderProxySession()
        let first = try pool.session(proxyURL: "http://127.0.0.1:\(try XCTUnwrap(firstServer.port))", fallback: fallback)
        let second = try pool.session(proxyURL: "http://127.0.0.1:\(try XCTUnwrap(secondServer.port))", fallback: fallback)
        defer { first.invalidateAndCancel(); second.invalidateAndCancel() }
        var request = URLRequest(url: URL(string: "https://account-proxy-test.invalid/")!)
        request.timeoutInterval = 3
        _ = try? await first.data(for: request)
        _ = try? await second.data(for: request)
        await fulfillment(of: [firstReceived, secondReceived], timeout: 2)
    }

    func testSupportedProtocols() throws {
        for scheme in ["http", "https", "socks5"] {
            XCTAssertNotNil(try ProviderProxySession.proxyConfiguration(for: "\(scheme)://127.0.0.1:1080"))
        }
    }

    func testSessionsAreIsolatedAndSystemDefaultsRemainUnchanged() throws {
        let pool = ProviderProxySession()
        let fallback = URLSession(configuration: .ephemeral)
        defer { fallback.invalidateAndCancel() }
        XCTAssertTrue(try pool.session(proxyURL: "  ", fallback: fallback) === fallback)
        let first = try pool.session(proxyURL: "http://127.0.0.1:8080", fallback: fallback)
        let second = try pool.session(proxyURL: "socks5://127.0.0.1:1080", fallback: fallback)
        defer { first.invalidateAndCancel(); second.invalidateAndCancel() }
        XCTAssertFalse(first === second)
        XCTAssertTrue(try pool.session(proxyURL: "http://127.0.0.1:8080", fallback: fallback) === first)
        XCTAssertEqual(first.configuration.proxyConfigurations.count, 1)
        XCTAssertFalse(first.configuration.proxyConfigurations[0].allowFailover)
        XCTAssertEqual(second.configuration.proxyConfigurations.count, 1)
        XCTAssertTrue(fallback.configuration.proxyConfigurations.isEmpty)
    }
}
