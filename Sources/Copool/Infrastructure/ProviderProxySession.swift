import Foundation
import Network

/// Reuses isolated sessions without changing process-wide or system proxy settings.
final class ProviderProxySession: @unchecked Sendable {
    static let shared = ProviderProxySession()

    private let lock = NSLock()
    private let sessions = NSCache<NSString, URLSession>()

    init() {
        sessions.countLimit = 32
    }

    static func proxyConfiguration(for value: String) throws -> Network.ProxyConfiguration? {
        let value = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        guard let url = URLComponents(string: value),
              let scheme = url.scheme?.lowercased(),
              ["http", "https", "socks5"].contains(scheme),
              let host = url.host, !host.isEmpty,
              let port = url.port, (1...65535).contains(port),
              url.user == nil, url.password == nil,
              url.query == nil, url.fragment == nil,
              url.path.isEmpty || url.path == "/" else {
            throw AppError.invalidData(L10n.tr("error.sub2api.proxy_invalid"))
        }
        let endpoint = NWEndpoint.hostPort(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: UInt16(port))!)
        var proxy: Network.ProxyConfiguration
        if scheme == "socks5" {
            proxy = Network.ProxyConfiguration(socksv5Proxy: endpoint)
        } else {
            proxy = Network.ProxyConfiguration(
                httpCONNECTProxy: endpoint,
                tlsOptions: scheme == "https" ? NWProtocolTLS.Options() : nil
            )
        }
        proxy.allowFailover = false
        return proxy
    }

    func session(proxyURL: String, fallback: URLSession) throws -> URLSession {
        guard let proxy = try Self.proxyConfiguration(for: proxyURL) else { return fallback }
        let key = "\(ObjectIdentifier(fallback))|\(proxyURL.trimmingCharacters(in: .whitespacesAndNewlines))" as NSString
        lock.lock()
        defer { lock.unlock() }
        if let cached = sessions.object(forKey: key) { return cached }
        let configuration = fallback.configuration
        configuration.proxyConfigurations = [proxy]
        let session = URLSession(configuration: configuration, delegate: fallback.delegate, delegateQueue: nil)
        sessions.setObject(session, forKey: key)
        return session
    }
}
