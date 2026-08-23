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
        settings.sub2APIProvider = Sub2APISettingsConfiguration(
            confirmedProviderIDs: ["my"],
            providers: [
                Sub2APIProviderConfiguration(
                    providerID: " my ",
                    username: " admin@example.com ",
                    password: "secret",
                    allowInsecureTLS: true,
                    importedAccountIDs: [2, 1, 2]
                )
            ]
        )

        let decoded = try JSONDecoder().decode(
            AppSettings.self,
            from: JSONEncoder().encode(settings)
        )

        XCTAssertEqual(
            decoded.sub2APIProvider,
            Sub2APISettingsConfiguration(
                confirmedProviderIDs: ["my"],
                providers: [
                    Sub2APIProviderConfiguration(
                        id: settings.sub2APIProvider.providers[0].id,
                        providerID: "my",
                        username: "admin@example.com",
                        password: "secret",
                        allowInsecureTLS: true,
                        importedAccountIDs: [1, 2]
                    )
                ]
            )
        )
    }

    func testLegacySingleConfigurationMigratesWithoutImplicitConfirmation() throws {
        let data = Data(#"{"isEnabled":true,"providerID":"my","adminBaseURL":"https://sub2.test/api/v1","username":"admin@example.com","password":"secret","allowInsecureTLS":false}"#.utf8)

        let configuration = try JSONDecoder().decode(Sub2APISettingsConfiguration.self, from: data).normalized()

        XCTAssertEqual(configuration.providers.first?.providerID, "my")
        XCTAssertEqual(configuration.providers.first?.legacyAdminBaseURL, "https://sub2.test/api/v1")
        XCTAssertFalse(configuration.confirms(providerID: "my"))
    }

    func testCompleteSub2APICredentialsEnableConnectionRegardlessOfLegacyToggle() throws {
        let configuration = Sub2APIProviderConfiguration(
            providerID: "my",
            username: "admin@example.com",
            password: "secret"
        ).normalized()

        XCTAssertTrue(configuration.isEnabled)
        XCTAssertTrue(configuration.isComplete)
    }
}
