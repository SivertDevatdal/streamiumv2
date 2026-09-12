import Foundation
import StreamiumCore

/// The app's model: sources, the merged catalog, favourites and the guide.
/// All published properties change on the main actor; heavy work runs in
/// detached tasks and on the Rust side.
@MainActor
public final class Library: ObservableObject {
    @Published public private(set) var sources: [SourceRecord] = []
    @Published public private(set) var channels: [FfiChannel] = []
    @Published public private(set) var groups: [String] = []
    @Published public private(set) var favouriteIDs: Set<String> = []
    @Published public private(set) var isRefreshing = false
    @Published public private(set) var guideProgrammeCount: UInt64 = 0
    @Published public var preferHLSForXtream = false

    public let guide = EpgGuide()
    private let catalog: ChannelCatalog
    private let store: SourceStore
    private let loader: SourceLoader
    /// Channel id → EPG channel id, filled lazily as rows appear on screen.
    private var epgResolution: [String: String?] = [:]

    public init(store: SourceStore = .shared, loader: SourceLoader = SourceLoader()) {
        self.store = store
        self.loader = loader
        if let json = store.loadCatalogJSON(), let restored = try? ChannelCatalog.fromJson(json: json) {
            catalog = restored
        } else {
            catalog = ChannelCatalog()
        }
        sources = store.loadSources()
        publishCatalog()
        Task { await loadCachedGuides() }
    }

    // MARK: sources

    public func addPlaylist(name: String, url: String, epgURL: String?) async throws {
        var record = SourceRecord(kind: .playlist, name: name.isEmpty ? url : name, location: url, epgURL: epgURL)
        try await refresh(&record)
        sources.append(record)
        store.saveSources(sources)
    }

    public func addXtream(name: String, server: String, username: String, password: String) async throws {
        var record = SourceRecord(kind: .xtream, name: name.isEmpty ? server : name, location: server, username: username)
        store.setSecret(password, for: record.id)
        do {
            try await refresh(&record)
        } catch {
            store.setSecret(nil, for: record.id)
            throw error
        }
        sources.append(record)
        store.saveSources(sources)
    }

    public func remove(_ source: SourceRecord) {
        sources.removeAll { $0.id == source.id }
        store.setSecret(nil, for: source.id)
        store.removeCache(for: "playlist-\(source.id)")
        store.saveSources(sources)
        Task { await rebuildCatalogFromCache() }
    }

    public func refreshAll() async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        for index in sources.indices {
            var record = sources[index]
            do {
                try await refresh(&record)
            } catch {
                record.lastError = error.localizedDescription
            }
            sources[index] = record
        }
        store.saveSources(sources)
    }

    private func refresh(_ record: inout SourceRecord) async throws {
        let result: SourceLoader.Result
        switch record.kind {
        case .playlist:
            result = try await loader.loadPlaylist(url: record.location, epgURL: record.epgURL)
        case .xtream:
            guard let password = store.secret(for: record.id) else {
                throw SourceLoaderError.xtream("Password missing; remove and re-add the source")
            }
            result = try await loader.loadXtream(
                server: record.location, username: record.username ?? "",
                password: password, preferHLS: preferHLSForXtream
            )
        case .localFolder:
            return
        }
        record.lastRefreshed = Date()
        record.lastError = nil

        // Cache the parsed channels so the next launch is instant and offline.
        let payload = try JSONEncoder().encode(result.channels.map(CachedChannel.init))
        store.cache(payload, for: "playlist-\(record.id)")
        for (i, doc) in result.epgDocuments.enumerated() {
            store.cache(doc, for: "epg-\(record.id)-\(i)")
        }
        await rebuildCatalogFromCache(including: (record.id, result.channels))
        for doc in result.epgDocuments {
            await loadGuide(doc)
        }
    }

    // MARK: catalog

    private func rebuildCatalogFromCache(including fresh: (UUID, [FfiChannel])? = nil) async {
        let ids = sources.map(\.id) + (fresh.map { [$0.0] } ?? [])
        let store = self.store
        let all: [FfiChannel] = await Task.detached(priority: .userInitiated) {
            var out: [FfiChannel] = []
            for id in Set(ids) {
                if let fresh, fresh.0 == id {
                    out.append(contentsOf: fresh.1)
                } else if let data = store.cachedData(for: "playlist-\(id)"),
                          let cached = try? JSONDecoder().decode([CachedChannel].self, from: data) {
                    out.append(contentsOf: cached.map(\.channel))
                }
            }
            return out
        }.value
        let favourites = favouriteIDs
        catalog.clear()
        catalog.addChannels(channels: all)
        for id in favourites { catalog.setFavourite(id: id, on: true) }
        publishCatalog()
        store.saveCatalogJSON(catalog.toJson())
    }

    private func publishCatalog() {
        channels = catalog.channels()
        groups = catalog.groups()
        favouriteIDs = Set(catalog.favourites().map(\.id))
        epgResolution.removeAll()
    }

    public func channels(inGroup group: String?) -> [FfiChannel] {
        guard let group else { return channels }
        return catalog.inGroup(group: group)
    }

    public func search(_ query: String, limit: UInt32 = 200) -> [FfiChannel] {
        catalog.search(query: query, limit: limit)
    }

    public func channel(withID id: String) -> FfiChannel? {
        catalog.get(id: id)
    }

    public func channel(number: UInt32) -> FfiChannel? {
        catalog.byNumber(number: number)
    }

    public func toggleFavourite(_ channel: FfiChannel) {
        let on = !favouriteIDs.contains(channel.id)
        catalog.setFavourite(id: channel.id, on: on)
        if on { favouriteIDs.insert(channel.id) } else { favouriteIDs.remove(channel.id) }
        store.saveCatalogJSON(catalog.toJson())
    }

    public var favourites: [FfiChannel] { catalog.favourites() }

    // MARK: guide

    private func loadCachedGuides() async {
        for source in sources {
            var i = 0
            while let doc = store.cachedData(for: "epg-\(source.id)-\(i)") {
                await loadGuide(doc)
                i += 1
            }
        }
    }

    private func loadGuide(_ document: Data) async {
        let guide = self.guide
        let count: UInt64? = await Task.detached(priority: .utility) {
            try? guide.loadXmltv(bytes: document)
        }.value
        if let count {
            guideProgrammeCount = count
            epgResolution.removeAll()
        }
    }

    /// Now/next for a channel, or nil when the guide has nothing for it.
    public func nowNext(for channel: FfiChannel, at date: Date = Date()) -> FfiNowNext? {
        let epgID: String?
        if let cached = epgResolution[channel.id] {
            epgID = cached
        } else {
            epgID = guide.resolve(tvgId: channel.epgId, name: channel.name)
            epgResolution[channel.id] = epgID
        }
        guard let epgID else { return nil }
        let result = guide.nowNext(channelId: epgID, atUnix: Int64(date.timeIntervalSince1970))
        return (result.now == nil && result.next == nil) ? nil : result
    }

    public func programmes(for channel: FfiChannel, from: Date, to: Date) -> [FfiProgramme] {
        guard let epgID = guide.resolve(tvgId: channel.epgId, name: channel.name) else { return [] }
        return guide.between(
            channelId: epgID,
            fromUnix: Int64(from.timeIntervalSince1970),
            toUnix: Int64(to.timeIntervalSince1970)
        )
    }
}

/// Codable mirror of `FfiChannel` for the on-disk cache.
struct CachedChannel: Codable {
    var id: String
    var name: String
    var url: String
    var kind: String
    var format: String
    var group: String?
    var logo: String?
    var epgId: String?
    var number: UInt32?
    var catchupKind: String?
    var catchupSource: String?
    var catchupDays: UInt32?
    var userAgent: String?
    var referrer: String?
    var headers: [String: String]

    init(_ c: FfiChannel) {
        id = c.id
        name = c.name
        url = c.url
        kind = "\(c.kind)"
        format = "\(c.format)"
        group = c.group
        logo = c.logo
        epgId = c.epgId
        number = c.number
        catchupKind = c.catchupKind
        catchupSource = c.catchupSource
        catchupDays = c.catchupDays
        userAgent = c.userAgent
        referrer = c.referrer
        var h: [String: String] = [:]
        for x in c.headers { h[x.name] = x.value }
        headers = h
    }

    var channel: FfiChannel {
        let kindValue: FfiMediaKind = {
            switch kind {
            case "movie": return .movie
            case "episode": return .episode
            case "radio": return .radio
            case "personal": return .personal
            default: return .live
            }
        }()
        let formatValue: FfiStreamFormat = {
            switch format {
            case "hls": return .hls
            case "dash": return .dash
            case "mpegTs": return .mpegTs
            case "progressive": return .progressive
            case "rtsp": return .rtsp
            case "udp": return .udp
            default: return .unknown
            }
        }()
        return FfiChannel(
            id: id, name: name, url: url, kind: kindValue, format: formatValue, group: group, logo: logo,
            epgId: epgId, number: number, catchupKind: catchupKind, catchupSource: catchupSource,
            catchupDays: catchupDays, userAgent: userAgent, referrer: referrer,
            headers: headers.map { FfiHeader(name: $0.key, value: $0.value) }.sorted { $0.name < $1.name }
        )
    }
}
