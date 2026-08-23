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
}
