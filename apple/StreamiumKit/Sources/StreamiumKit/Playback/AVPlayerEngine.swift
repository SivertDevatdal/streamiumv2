import AVFoundation
import Foundation
import QuartzCore

/// Engine for everything AVPlayer handles natively: HLS, MP4/MOV, audio
/// files, plus AirPlay, Picture in Picture and FairPlay for free.
@MainActor
public final class AVPlayerEngine: NSObject, PlayerEngine {
    public let playerLayer = AVPlayerLayer()
    public var layer: CALayer { playerLayer }
    public let player = AVPlayer()

    public private(set) var state: PlaybackState = .idle {
        didSet { if state != oldValue { onStateChange?(state) } }
    }
    public private(set) var stats = PlaybackStats()
    public var onStateChange: ((PlaybackState) -> Void)?
    public var volume: Float {
        get { player.volume }
        set { player.volume = newValue }
    }

    private var observations: [NSKeyValueObservation] = []
    private var endObserver: NSObjectProtocol?
    private var loadStarted: Date?
    private var currentItem: AVPlayerItem?

    public override init() {
        super.init()
        playerLayer.player = player
        playerLayer.videoGravity = .resizeAspect
        player.automaticallyWaitsToMinimizeStalling = true
        #if os(iOS) || os(tvOS)
        player.allowsExternalPlayback = true
        #endif
    }

    public func load(_ item: PlaybackItem) {
        teardownObservers()
        stats = PlaybackStats()
        loadStarted = Date()
        state = .connecting

        var options: [String: Any] = [:]
        let headers = item.httpHeaders
        if !headers.isEmpty {
            options["AVURLAssetHTTPHeaderFieldsKey"] = headers
        }
        let asset = AVURLAsset(url: item.url, options: options)
        let playerItem = AVPlayerItem(asset: asset)
        playerItem.preferredForwardBufferDuration = item.isLive ? 3 : 0
        currentItem = playerItem
        observe(playerItem)
        player.replaceCurrentItem(with: playerItem)
    }

    public func play() {
        player.play()
        if state == .paused { state = .playing }
    }

    public func pause() {
        player.pause()
        if state == .playing { state = .paused }
    }

    public func stop() {
        player.pause()
        player.replaceCurrentItem(with: nil)
        teardownObservers()
        currentItem = nil
        state = .idle
    }

    private func observe(_ item: AVPlayerItem) {
        observations = [
            item.observe(\.status, options: [.new]) { [weak self] item, _ in
                let status = item.status
                let message = item.error?.localizedDescription
                Task { @MainActor [weak self] in self?.handle(status: status, message: message) }
            },
            player.observe(\.timeControlStatus, options: [.new]) { [weak self] player, _ in
                let tcs = player.timeControlStatus
                Task { @MainActor [weak self] in self?.handle(timeControl: tcs) }
            },
            item.observe(\.loadedTimeRanges, options: [.new]) { [weak self] item, _ in
                let ranges = item.loadedTimeRanges.map { $0.timeRangeValue }
                let position = item.currentTime()
                Task { @MainActor [weak self] in self?.updateBuffer(ranges: ranges, position: position) }
            },
        ]
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in self?.state = .ended }
        }
    }

    private func teardownObservers() {
        observations.forEach { $0.invalidate() }
        observations.removeAll()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
    }

    private func handle(status: AVPlayerItem.Status, message: String?) {
        switch status {
        case .failed:
            state = .failed(message ?? "Playback failed")
        case .readyToPlay:
            if state == .connecting { state = .buffering }
        default:
            break
        }
    }

    private func handle(timeControl: AVPlayer.TimeControlStatus) {
        switch timeControl {
        case .playing:
            if stats.timeToFirstFrame == nil, let start = loadStarted {
                stats.timeToFirstFrame = Date().timeIntervalSince(start)
            }
            state = .playing
        case .waitingToPlayAtSpecifiedRate:
            if state == .playing { stats.rebuffers += 1 }
            if state != .connecting { state = .buffering }
        case .paused:
            if state == .playing { state = .paused }
        @unknown default:
            break
        }
    }

    private func updateBuffer(ranges: [CMTimeRange], position: CMTime) {
        guard let range = ranges.first(where: { $0.containsTime(position) }) ?? ranges.last else { return }
        let ahead = CMTimeSubtract(CMTimeRangeGetEnd(range), position)
        stats.bufferedSeconds = max(0, CMTimeGetSeconds(ahead))
    }
}
