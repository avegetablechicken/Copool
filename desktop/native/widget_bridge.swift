import Foundation
import WidgetKit

// WidgetKit remains a small native host adapter; all account/usage state is Rust-owned.
@_cdecl("copool_publish_widget")
public func copoolPublishWidget(
    _ filename: UnsafePointer<CChar>?,
    _ kind: UnsafePointer<CChar>?,
    _ bytes: UnsafePointer<UInt8>?,
    _ length: Int
) -> Int32 {
    guard let filename, let kind, let bytes, length > 0, length <= 1_048_576 else { return 1 }
    let data = Data(bytes: bytes, count: length)
    guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return 2 }
    // The build records the app group it signed the app for.
    guard let group = Bundle.main.object(forInfoDictionaryKey: "CopoolAppGroup") as? String, !group.isEmpty,
          let folder = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else { return 3 }
    do {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try data.write(to: folder.appendingPathComponent(String(cString: filename)), options: .atomic)
        UserDefaults(suiteName: group)?.set(object["usageProgressDisplayMode"] as? String ?? "used", forKey: "accountsWidgetUsageProgressDisplayMode")
        WidgetCenter.shared.reloadTimelines(ofKind: String(cString: kind))
        return 0
    } catch { return 4 }
}
