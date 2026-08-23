import Foundation

struct CodexModelProviderDefinition: Equatable, Sendable {
    var id: String
    var baseURL: String?

    var isOfficialOpenAI: Bool {
        id.caseInsensitiveCompare("openai") == .orderedSame
    }
}

enum CodexModelProviderResolver {
    static func resolve(configPath: URL) -> CodexModelProviderDefinition {
        guard let raw = try? String(contentsOf: configPath, encoding: .utf8), !raw.isEmpty else {
            return CodexModelProviderDefinition(id: "openai", baseURL: nil)
        }
        return resolve(raw: raw)
    }

    static func resolve(raw: String) -> CodexModelProviderDefinition {
        let document = CodexConfigDocument(raw: raw)
        let providerID = document.activeProfile
            .flatMap { document.profileModelProviders[$0] }
            ?? document.defaultModelProvider
            ?? "openai"
        return CodexModelProviderDefinition(
            id: providerID,
            baseURL: document.modelProviderBaseURLs.first {
                $0.key.caseInsensitiveCompare(providerID) == .orderedSame
            }?.value
        )
    }

    static func definitions(configPath: URL) -> [CodexModelProviderDefinition] {
        guard let raw = try? String(contentsOf: configPath, encoding: .utf8), !raw.isEmpty else {
            return []
        }
        let document = CodexConfigDocument(raw: raw)
        return document.modelProviderBaseURLs.map { providerID, baseURL in
            CodexModelProviderDefinition(id: providerID, baseURL: baseURL)
        }
    }

    static func providerID(matchingBaseURL rawURL: String, configPath: URL) -> String? {
        guard let targetOrigin = originKey(rawURL) else { return nil }
        let matches = definitions(configPath: configPath).filter { definition in
            guard let baseURL = definition.baseURL else { return false }
            return originKey(baseURL) == targetOrigin
        }
        return matches.count == 1 ? matches[0].id : nil
    }

    private static func originKey(_ rawURL: String) -> String? {
        guard let components = URLComponents(string: rawURL),
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased() else {
            return nil
        }
        let port: Int?
        if let explicitPort = components.port {
            port = explicitPort
        } else if scheme == "https" {
            port = 443
        } else if scheme == "http" {
            port = 80
        } else {
            port = nil
        }
        return "\(scheme)://\(host):\(port.map(String.init) ?? "")"
    }
}

final class CodexModelProviderSwitchService: CodexModelProviderSwitchServiceProtocol, @unchecked Sendable {
    private let configPath: URL
    private let fileManager: FileManager

    init(configPath: URL, fileManager: FileManager = .default) {
        self.configPath = configPath
        self.fileManager = fileManager
    }

    func switchProvider(to providerID: String) throws {
        let providerID = providerID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !providerID.isEmpty else {
            throw AppError.invalidData(L10n.tr("error.sub2api.provider_not_confirmed"))
        }

        let raw: String
        if fileManager.fileExists(atPath: configPath.path) {
            raw = try String(contentsOf: configPath, encoding: .utf8)
        } else {
            raw = ""
        }
        let updated = try CodexConfigDocument.updatingModelProvider(in: raw, to: providerID)
        let directory = configPath.deletingLastPathComponent()
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let existingPermissions = try? fileManager.attributesOfItem(atPath: configPath.path)[.posixPermissions]
        try Data(updated.utf8).write(to: configPath, options: .atomic)
        if let existingPermissions {
            try fileManager.setAttributes(
                [.posixPermissions: existingPermissions],
                ofItemAtPath: configPath.path
            )
        }
    }
}

enum ChatGPTBaseOriginResolver {
    static func resolve(configPath: URL) -> String {
        guard let raw = try? String(contentsOf: configPath, encoding: .utf8), !raw.isEmpty else {
            return "https://chatgpt.com"
        }

        for line in raw.split(whereSeparator: { $0.isNewline }) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("chatgpt_base_url") else { continue }
            guard let equalIndex = trimmed.firstIndex(of: "=") else { continue }
            let value = trimmed[trimmed.index(after: equalIndex)...]
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            if !value.isEmpty {
                return value.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            }
        }

        return "https://chatgpt.com"
    }
}

private struct CodexConfigDocument {
    var defaultModelProvider: String?
    var activeProfile: String?
    var profileModelProviders: [String: String] = [:]
    var modelProviderBaseURLs: [String: String] = [:]

    init(raw: String) {
        var section: [String] = []

        for rawLine in raw.split(whereSeparator: { $0.isNewline }) {
            let line = Self.strippingComment(from: String(rawLine))
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !line.isEmpty else { continue }

            if line.hasPrefix("["), line.hasSuffix("]") {
                section = Self.parsePath(String(line.dropFirst().dropLast()))
                continue
            }

            guard let assignment = Self.parseAssignment(line) else { continue }
            switch section {
            case []:
                if assignment.key == "model_provider" {
                    defaultModelProvider = assignment.value
                } else if assignment.key == "profile" {
                    activeProfile = assignment.value
                }
            case let path where path.count == 2 && path[0] == "profiles":
                if assignment.key == "model_provider" {
                    profileModelProviders[path[1]] = assignment.value
                }
            case let path where path.count == 2 && path[0] == "model_providers":
                if assignment.key == "base_url" {
                    modelProviderBaseURLs[path[1]] = assignment.value
                        .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                }
            default:
                continue
            }
        }
    }

    static func updatingModelProvider(in raw: String, to providerID: String) throws -> String {
        let document = CodexConfigDocument(raw: raw)
        let targetSection: [String]
        if let profile = document.activeProfile,
           document.profileModelProviders[profile] != nil {
            targetSection = ["profiles", profile]
        } else {
            targetSection = []
        }

        var lines = raw.components(separatedBy: "\n")
        if lines.count == 1, lines[0].isEmpty {
            lines = []
        }
        var section: [String] = []
        var insertionIndex = lines.last == "" ? max(0, lines.count - 1) : lines.count

        for index in lines.indices {
            let rawLine = lines[index]
            let content = strippingComment(from: rawLine)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if content.hasPrefix("["), content.hasSuffix("]") {
                if targetSection.isEmpty, insertionIndex == lines.count {
                    insertionIndex = index
                }
                section = parsePath(String(content.dropFirst().dropLast()))
                continue
            }

            guard section == targetSection,
                  let assignment = parseAssignment(content),
                  assignment.key == "model_provider" else {
                continue
            }
            lines[index] = replacingAssignmentValue(
                in: rawLine,
                key: "model_provider",
                value: providerID
            )
            return preservingTrailingNewline(of: raw, in: lines)
        }

        guard targetSection.isEmpty else {
            throw AppError.invalidData(L10n.tr("error.sub2api.provider_not_confirmed"))
        }
        lines.insert("model_provider = \(quoted(providerID))", at: insertionIndex)
        return preservingTrailingNewline(of: raw, in: lines)
    }

    private static func parseAssignment(_ line: String) -> (key: String, value: String)? {
        guard let index = firstUnquotedCharacter("=", in: line) else { return nil }
        let key = String(line[..<index]).trimmingCharacters(in: .whitespacesAndNewlines)
        let value = String(line[line.index(after: index)...]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, let parsedValue = parseStringValue(value) else { return nil }
        return (unquote(key), parsedValue)
    }

    private static func parseStringValue(_ value: String) -> String? {
        let parsed = unquote(value).trimmingCharacters(in: .whitespacesAndNewlines)
        return parsed.isEmpty ? nil : parsed
    }

    private static func parsePath(_ path: String) -> [String] {
        var components: [String] = []
        var current = ""
        var quote: Character?

        for character in path {
            if character == "\"" || character == "'" {
                if quote == character {
                    quote = nil
                } else if quote == nil {
                    quote = character
                } else {
                    current.append(character)
                }
            } else if character == ".", quote == nil {
                let value = unquote(current.trimmingCharacters(in: .whitespacesAndNewlines))
                if !value.isEmpty { components.append(value) }
                current = ""
            } else {
                current.append(character)
            }
        }

        let value = unquote(current.trimmingCharacters(in: .whitespacesAndNewlines))
        if !value.isEmpty { components.append(value) }
        return components
    }

    private static func strippingComment(from line: String) -> String {
        guard let index = firstUnquotedCharacter("#", in: line) else { return line }
        return String(line[..<index])
    }

    private static func firstUnquotedCharacter(_ target: Character, in value: String) -> String.Index? {
        var quote: Character?
        var isEscaped = false

        for index in value.indices {
            let character = value[index]
            if isEscaped {
                isEscaped = false
                continue
            }
            if character == "\\", quote == "\"" {
                isEscaped = true
                continue
            }
            if character == "\"" || character == "'" {
                if quote == character {
                    quote = nil
                } else if quote == nil {
                    quote = character
                }
                continue
            }
            if character == target, quote == nil {
                return index
            }
        }
        return nil
    }

    private static func unquote(_ value: String) -> String {
        guard value.count >= 2, let first = value.first, let last = value.last,
              (first == "\"" || first == "'"), first == last else {
            return value
        }
        return String(value.dropFirst().dropLast())
    }

    private static func replacingAssignmentValue(
        in line: String,
        key: String,
        value: String
    ) -> String {
        let indentation = String(line.prefix { $0 == " " || $0 == "\t" })
        let comment = firstUnquotedCharacter("#", in: line).map { index in
            String(line[index...])
        }
        let suffix = comment.map { " \($0)" } ?? ""
        return "\(indentation)\(key) = \(quoted(value))\(suffix)"
    }

    private static func quoted(_ value: String) -> String {
        let escaped = value
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }

    private static func preservingTrailingNewline(of original: String, in lines: [String]) -> String {
        let joined = lines.joined(separator: "\n")
        guard original.hasSuffix("\n"), !joined.hasSuffix("\n") else { return joined }
        return joined + "\n"
    }
}
