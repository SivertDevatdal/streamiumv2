import Foundation
import QuartzCore
import StreamiumCore

/// Observable playback state shared by every engine.
public enum PlaybackState: Equatable {
    case idle
    case connecting
    case buffering
    case playing
    case paused
    case ended
    case failed(String)

    public var isActive: Bool {
        switch self {
        case .connecting, .buffering, .playing, .paused: return true
        default: return false
        }
    }
}

/// Live diagnostics surfaced in the debug overlay and in bug reports.
public struct PlaybackStats: Equatable {
    public var timeToFirstFrame: TimeInterval?
    public var bufferedSeconds: Double = 0
    public var droppedFrames: Int = 0
    public var rebuffers: Int = 0
    public var discontinuities: Int = 0
    public var bytesReceived: Int64 = 0
    public var videoCodec: String?
    public var audioCodec: String?
    public init() {}
}

/// Everything an engine needs to open one stream.
public struct PlaybackItem: Equatable {
    public var url: URL
    public var title: String
    public var format: FfiStreamFormat
    public var isLive: Bool
    public var userAgent: String?
    public var referrer: String?
    public var headers: [String: String]

    public init(
        url: URL,
        title: String,
        format: FfiStreamFormat = .unknown,
        isLive: Bool = true,
        userAgent: String? = nil,
        referrer: String? = nil,
        headers: [String: String] = [:]
    ) {
        self.url = url
        self.title = title
        self.format = format
        self.isLive = isLive
        self.userAgent = userAgent
        self.referrer = referrer
        self.headers = headers
    }

    /// Build from a catalog channel. Returns nil when the URL is unusable.
    public init?(channel: FfiChannel) {
        guard let url = URL(string: channel.url.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            return nil
        }
        var headers: [String: String] = [:]
        for h in channel.headers { headers[h.name] = h.value }
        self.init(
            url: url,
            title: channel.name,
            format: channel.format,
            isLive: channel.kind == .live || channel.kind == .radio,
            userAgent: channel.userAgent,
            referrer: channel.referrer,
            headers: headers
        )
    }

    /// HTTP headers to send, with User-Agent and Referer folded in.
    public var httpHeaders: [String: String] {
        var h = headers
        if let ua = userAgent, !ua.isEmpty { h["User-Agent"] = ua }
        if let r = referrer, !r.isEmpty { h["Referer"] = r }
        return h
    }
}

/// A playback engine owns a video surface and plays one item at a time.
/// Engines are main-actor objects; they do their heavy lifting on private
/// queues and hop back to the main actor to report state.
@MainActor
public protocol PlayerEngine: AnyObject {
    /// The layer to install in the player view.
    var layer: CALayer { get }
    var state: PlaybackState { get }
    var stats: PlaybackStats { get }
    var onStateChange: ((PlaybackState) -> Void)? { get set }
    var volume: Float { get set }

    func load(_ item: PlaybackItem)
    func play()
    func pause()
    func stop()
}
