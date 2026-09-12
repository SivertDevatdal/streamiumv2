import Foundation
import StreamiumCore

public enum SourceLoaderError: LocalizedError {
    case badURL(String)
    case http(Int)
    case notAPlaylist
    case xtream(String)
    case core(String)

    public var errorDescription: String? {
        switch self {
        case .badURL(let s): return "Invalid address: \(s)"
        case .http(let code): return "Server answered HTTP \(code)"
        case .notAPlaylist: return "The address did not return a playlist"
        case .xtream(let m): return m
        case .core(let m): return m
        }
    }
}

/// Fetches documents with URLSession and hands them to the Rust core. The
/// core never does I/O, so this is the only place that talks to the network
/// for catalog data.
public struct SourceLoader {
    public struct Result {
        public var channels: [FfiChannel]
        public var epgURLs: [String]
        public var warnings: [String]
        public var epgDocuments: [Data]
    }

    private let session: URLSession

    public init(session: URLSession = SourceLoader.defaultSession) {
        self.session = session
    }

    public static let defaultSession: URLSession = {
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 30
        config.timeoutIntervalForResource = 600
        config.httpAdditionalHeaders = ["User-Agent": "Streamium/\(coreVersion())"]
        return URLSession(configuration: config)
    }()

    public func fetchData(_ urlString: String) async throws -> Data {
        let trimmed = urlString.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed) else { throw SourceLoaderError.badURL(urlString) }
        if url.isFileURL {
            return try Data(contentsOf: url)
        }
        let (data, response) = try await session.data(from: url)
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            throw SourceLoaderError.http(http.statusCode)
        }
        return data
    }

    public func fetchText(_ urlString: String) async throws -> String {
        let data = try await fetchData(urlString)
        return String(decoding: data, as: UTF8.self)
    }

    // MARK: playlists

    public func loadPlaylist(url: String, epgURL: String?) async throws -> Result {
        let text = try await fetchText(url)
        let playlist: FfiPlaylist
        do {
            playlist = try parsePlaylist(text: text)
        } catch FfiError.NotAPlaylist(_) {
            throw SourceLoaderError.notAPlaylist
        } catch {
            throw SourceLoaderError.core(error.localizedDescription)
        }
        if playlist.isHlsMedia {
            // A single-stream HLS playlist: expose it as one channel.
            let one = FfiChannel(
                id: url, name: url, url: url, kind: .live, format: .hls, group: nil, logo: nil,
                epgId: nil, number: nil, catchupKind: nil, catchupSource: nil, catchupDays: nil,
                userAgent: nil, referrer: nil, headers: []
            )
            return Result(channels: [one], epgURLs: [], warnings: [], epgDocuments: [])
        }
        var epgURLs = playlist.epgUrls
        if let epgURL, !epgURL.isEmpty { epgURLs.insert(epgURL, at: 0) }
        let docs = await fetchGuides(epgURLs)
        return Result(channels: playlist.channels, epgURLs: epgURLs, warnings: playlist.warnings, epgDocuments: docs)
    }

    // MARK: Xtream

    public func loadXtream(server: String, username: String, password: String, preferHLS: Bool) async throws -> Result {
        let endpoints: XtreamEndpoints
        do {
            endpoints = try XtreamEndpoints(baseUrl: server, username: username, password: password)
        } catch {
            throw SourceLoaderError.badURL(server)
        }
        let accountJSON = try await fetchText(endpoints.accountInfo())
        let account: FfiAccount
        do {
            account = try xtreamDecodeAccount(json: accountJSON)
        } catch {
            throw SourceLoaderError.xtream("The server did not answer like an Xtream server")
        }
        guard account.isActive else {
            throw SourceLoaderError.xtream("Account is not active (\(account.status ?? "unknown status"))")
        }

        async let liveCategories = fetchText(endpoints.liveCategories())
        async let liveStreams = fetchText(endpoints.liveStreams(categoryId: nil))
        async let vodCategories = fetchText(endpoints.vodCategories())
        async let vodStreams = fetchText(endpoints.vodStreams(categoryId: nil))

        let liveCategoriesJSON = try await liveCategories
        let liveStreamsJSON = try await liveStreams
        let live = try xtreamLiveChannels(
            endpoints: endpoints, categoriesJson: liveCategoriesJSON,
            streamsJson: liveStreamsJSON, hls: preferHLS
        )
        var channels = live
        if let vc = try? await vodCategories, let vs = try? await vodStreams,
           let vod = try? xtreamVodChannels(endpoints: endpoints, categoriesJson: vc, streamsJson: vs) {
            channels.append(contentsOf: vod)
        }
        let docs = await fetchGuides([endpoints.xmltv()])
        return Result(channels: channels, epgURLs: [endpoints.xmltv()], warnings: [], epgDocuments: docs)
    }

    private func fetchGuides(_ urls: [String]) async -> [Data] {
        await withTaskGroup(of: Data?.self) { group in
            for u in urls {
                group.addTask { try? await fetchData(u) }
            }
            var out: [Data] = []
            for await d in group { if let d { out.append(d) } }
            return out
        }
    }
}
