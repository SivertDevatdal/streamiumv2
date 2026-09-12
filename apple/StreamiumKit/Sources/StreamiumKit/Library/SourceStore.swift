import Foundation
import Security

/// A user-added source. Streamium ships none of these; every record is
/// created from user input. Passwords live in the Keychain, never in this
/// struct's persisted form.
public struct SourceRecord: Identifiable, Codable, Equatable, Hashable {
    public enum Kind: String, Codable { case playlist, xtream, localFolder }

    public var id: UUID
    public var kind: Kind
    public var name: String
    /// Playlist URL, Xtream server base URL, or a folder bookmark identifier.
    public var location: String
    public var epgURL: String?
    public var username: String?
    public var lastRefreshed: Date?
    public var lastError: String?

    public init(id: UUID = UUID(), kind: Kind, name: String, location: String,
                epgURL: String? = nil, username: String? = nil) {
        self.id = id
        self.kind = kind
        self.name = name
        self.location = location
        self.epgURL = epgURL
        self.username = username
    }
}

/// Persists sources (JSON in Application Support), secrets (Keychain) and
/// cached documents (playlists, guides) so the app opens instantly offline.
public final class SourceStore: @unchecked Sendable {
    public static let shared = SourceStore()

    private let fileManager = FileManager.default
    private let root: URL
    private let ioQueue = DispatchQueue(label: "app.streamium.store", qos: .utility)
    private let service = "app.streamium.sources"

    public init(directory: URL? = nil) {
        if let directory {
            root = directory
        } else {
            let base = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            root = base.appendingPathComponent("Streamium", isDirectory: true)
        }
        try? fileManager.createDirectory(at: root, withIntermediateDirectories: true)
        try? fileManager.createDirectory(at: root.appendingPathComponent("cache"), withIntermediateDirectories: true)
    }

    // MARK: sources

    public func loadSources() -> [SourceRecord] {
        let url = root.appendingPathComponent("sources.json")
        guard let data = try? Data(contentsOf: url) else { return [] }
        return (try? JSONDecoder().decode([SourceRecord].self, from: data)) ?? []
    }

    public func saveSources(_ sources: [SourceRecord]) {
        let url = root.appendingPathComponent("sources.json")
        ioQueue.async {
            if let data = try? JSONEncoder().encode(sources) {
                try? data.write(to: url, options: .atomic)
            }
        }
    }

    // MARK: catalog / favourites

    public func loadCatalogJSON() -> String? {
        let url = root.appendingPathComponent("catalog.json")
        return try? String(contentsOf: url, encoding: .utf8)
    }

    public func saveCatalogJSON(_ json: String) {
        let url = root.appendingPathComponent("catalog.json")
        ioQueue.async { try? json.write(to: url, atomically: true, encoding: .utf8) }
    }

    // MARK: cached documents (playlists, XMLTV)

    public func cacheURL(for key: String) -> URL {
        let safe = key.unicodeScalars.map { CharacterSet.alphanumerics.contains($0) ? Character($0) : "_" }
        return root.appendingPathComponent("cache").appendingPathComponent(String(safe))
    }

    public func cachedData(for key: String) -> Data? {
        try? Data(contentsOf: cacheURL(for: key))
    }

    public func cache(_ data: Data, for key: String) {
        let url = cacheURL(for: key)
        ioQueue.async { try? data.write(to: url, options: .atomic) }
    }

    public func removeCache(for key: String) {
        try? fileManager.removeItem(at: cacheURL(for: key))
    }

    // MARK: secrets

    public func setSecret(_ value: String?, for id: UUID) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: id.uuidString,
        ]
        SecItemDelete(query as CFDictionary)
        guard let value, let data = value.data(using: .utf8) else { return }
        var add = query
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(add as CFDictionary, nil)
    }

    public func secret(for id: UUID) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: id.uuidString,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }
}
