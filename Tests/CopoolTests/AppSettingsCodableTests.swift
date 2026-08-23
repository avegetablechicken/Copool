import XCTest
@testable import Copool

final class AppSettingsCodableTests: XCTestCase {
    func testDecodeSettingsRequiresFullCurrentShape() throws {
        let json = """
        {
          "launchAtStartup": true,
          "launchCodexAfterSwitch": true,
          "autoSmartSwitch": false,
          "syncOpencodeOpenaiAuth": false
        }
        """

        XCTAssertThrowsError(try JSONDecoder().decode(AppSettings.self, from: Data(json.utf8)))
    }

    func testDecodeSettingsWithoutUsageProgressDisplayModeDefaultsToUsed() throws {
        let json = """
        {
          "launchAtStartup": true,
          "launchCodexAfterSwitch": true,
          "autoSmartSwitch": false,
          "syncOpencodeOpenaiAuth": false,
          "localProxyHostAPIOnly": false,
          "restartEditorsOnSwitch": false,
          "restartEditorTargets": [],
          "autoStartApiProxy": false,
          "proxyConfiguration": {
            "preferredPortText": "4141",
            "cloudflared": {
              "enabled": false,
              "tunnelMode": "quick",
              "useHTTP2": false,
              "namedHostname": ""
            }
          },
          "remoteServers": [],
          "locale": "en"
        }
        """

        let settings = try JSONDecoder().decode(AppSettings.self, from: Data(json.utf8))

        XCTAssertEqual(settings.usageProgressDisplayMode, .used)
        XCTAssertEqual(settings.sub2APIProvider, .defaultValue)
    }

    func testSub2APIProviderConfigurationRoundTrips() throws {
        var settings = AppSettings.defaultValue
        settings.sub2APIProvider = Sub2APIProviderConfiguration(
            isEnabled: true,
            providerID: " my ",
            adminBaseURL: " https://sub2.test:6060/api/v1/ ",
            username: " admin@example.com ",
            password: "secret",
            allowInsecureTLS: true,
            importedAccountIDs: [2, 1, 2]
        )

        let decoded = try JSONDecoder().decode(
            AppSettings.self,
            from: JSONEncoder().encode(settings)
        )

        XCTAssertEqual(
            decoded.sub2APIProvider,
            Sub2APIProviderConfiguration(
                isEnabled: true,
                adminBaseURL: "https://sub2.test:6060/api/v1",
                username: "admin@example.com",
                password: "secret",
                allowInsecureTLS: true,
                importedAccountIDs: [1, 2]
            )
        )
    }

    func testLegacyProviderIDDoesNotImplicitlyConfirmSub2APIIdentity() throws {
        let data = Data(#"{"isEnabled":true,"providerID":"my","adminBaseURL":"https://sub2.test/api/v1","username":"admin@example.com","password":"secret","allowInsecureTLS":false}"#.utf8)

        let configuration = try JSONDecoder().decode(Sub2APIProviderConfiguration.self, from: data).normalized()

        XCTAssertEqual(configuration.providerID, "")
        XCTAssertFalse(configuration.confirms(providerID: "my"))
    }

    func testCompleteSub2APICredentialsEnableConnectionRegardlessOfLegacyToggle() throws {
        let data = Data(#"{"isEnabled":false,"adminBaseURL":"https://sub2.test/api/v1","username":"admin@example.com","password":"secret","allowInsecureTLS":false}"#.utf8)

        let configuration = try JSONDecoder().decode(Sub2APIProviderConfiguration.self, from: data).normalized()

        XCTAssertTrue(configuration.isEnabled)
        XCTAssertTrue(configuration.isComplete)
    }
}
