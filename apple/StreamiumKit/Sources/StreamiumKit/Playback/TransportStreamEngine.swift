import AVFoundation
import CoreMedia
import Foundation
import QuartzCore
import StreamiumCore

/// Plays raw MPEG transport streams over HTTP with hardware decoding and no
/// third-party decoder:
///
///     URLSession → TransportStreamDemuxer (Rust) → CMSampleBuffers →
///     AVSampleBufferDisplayLayer + AVSampleBufferAudioRenderer
///
/// See docs/PLAYBACK.md for the design.
@MainActor
public final class TransportStreamEngine: NSObject, PlayerEngine {
    public let displayLayer: AVSampleBufferDisplayLayer
    public var layer: CALayer { displayLayer }

    public private(set) var state: PlaybackState = .idle {
        didSet { if state != oldValue { onStateChange?(state) } }
    }
    public private(set) var stats = PlaybackStats()
    public var onStateChange: ((PlaybackState) -> Void)?
    public var volume: Float {
        get { pipeline.audioRenderer.volume }
        set { pipeline.audioRenderer.volume = newValue }
    }

    private let pipeline: TransportStreamPipeline
    private var session: URLSession?
    private var task: URLSessionDataTask?
    private var statusObservation: NSKeyValueObservation?

    public override init() {
        let layer = AVSampleBufferDisplayLayer()
        layer.videoGravity = .resizeAspect
        displayLayer = layer
        pipeline = TransportStreamPipeline(displayLayer: layer)
        super.init()
        pipeline.onEvent = { [weak self] event in
            Task { @MainActor [weak self] in self?.handle(event) }
        }
    }

    public func load(_ item: PlaybackItem) {
        stop()
        stats = PlaybackStats()
        state = .connecting
        pipeline.start(isLive: item.isLive)

        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = false
        config.timeoutIntervalForRequest = 15
        config.timeoutIntervalForResource = .infinity
        config.httpAdditionalHeaders = item.httpHeaders
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData

        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        queue.underlyingQueue = pipeline.queue
        let session = URLSession(configuration: config, delegate: pipeline, delegateQueue: queue)
        self.session = session

        var request = URLRequest(url: item.url)
        request.timeoutInterval = 15
        let task = session.dataTask(with: request)
        self.task = task
        pipeline.task = task
        task.resume()

        statusObservation = displayLayer.observe(\.status, options: [.new]) { [weak self] layer, _ in
            if layer.status == .failed {
                let message = layer.error?.localizedDescription ?? "Video renderer failed"
                Task { @MainActor [weak self] in self?.state = .failed(message) }
            }
        }
    }

    public func play() {
        pipeline.setPaused(false)
        if state == .paused { state = .playing }
    }

    public func pause() {
        pipeline.setPaused(true)
        if state == .playing { state = .paused }
    }

    public func stop() {
        task?.cancel()
        task = nil
        session?.invalidateAndCancel()
        session = nil
        statusObservation = nil
        pipeline.stop()
        state = .idle
    }

    private func handle(_ event: TransportStreamPipeline.Event) {
        switch event {
        case .connected:
            state = .buffering
        case .buffering:
            if state == .playing { stats.rebuffers += 1 }
            state = .buffering
        case .playing(let timeToFirstFrame):
            if stats.timeToFirstFrame == nil { stats.timeToFirstFrame = timeToFirstFrame }
            state = .playing
        case .streams(let video, let audio):
            stats.videoCodec = video
            stats.audioCodec = audio
        case .progress(let bytes, let buffered, let discontinuities):
            stats.bytesReceived = bytes
            stats.bufferedSeconds = buffered
            stats.discontinuities = discontinuities
        case .ended:
            state = .ended
        case .failed(let message):
            state = .failed(message)
        }
    }
}

/// Everything below runs on `queue` (a serial queue): network delivery,
/// demuxing, bitstream conversion and sample enqueueing. Nothing here touches
/// the main actor; results go out through `onEvent`.
final class TransportStreamPipeline: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    enum Event {
        case connected
        case buffering
        case playing(TimeInterval)
        case streams(video: String?, audio: String?)
        case progress(bytes: Int64, buffered: Double, discontinuities: Int)
        case ended
        case failed(String)
    }

    let queue = DispatchQueue(label: "app.streamium.ts-pipeline", qos: .userInteractive)
    let audioRenderer = AVSampleBufferAudioRenderer()
    var onEvent: ((Event) -> Void)?
    weak var task: URLSessionDataTask?

    private let displayLayer: AVSampleBufferDisplayLayer
    private let synchronizer = AVSampleBufferRenderSynchronizer()
    private var videoRenderer: AVSampleBufferVideoRenderer { displayLayer.sampleBufferRenderer }

    private var demuxer = TransportStreamDemuxer()
    private var isLive = true
    private var running = false
    private var paused = false
    private var startedAt = Date()

    private var videoPid: UInt16?
    private var audioPid: UInt16?
    private var videoCodec: FfiCodec = .other
    private var audioCodec: FfiCodec = .other
    private var videoFormat: CMVideoFormatDescription?
    private var audioFormat: CMAudioFormatDescription?
    private var h264Sets: H264ParameterSets?
    private var hevcSets: HEVCParameterSets?
    private var adtsHeader: ADTSHeader?
    private var ac3Info: AC3.FrameInfo?
    private var mpegInfo: MPEGAudio.FrameInfo?

    private var pts = PtsUnwrapper()
    private var firstPts: Int64?
    private var lastAudioPts: Int64?
    private var lastVideoPts: Int64?
    private var clockStarted = false
    private var bytesReceived: Int64 = 0
    private var discontinuities = 0
    private var suspended = false

    /// Seconds of media to accumulate before starting the clock.
    private var startupBuffer: Double = 1.5
    private static let timescale: CMTimeScale = 90_000

    init(displayLayer: AVSampleBufferDisplayLayer) {
        self.displayLayer = displayLayer
        super.init()
        synchronizer.addRenderer(videoRenderer)
        synchronizer.addRenderer(audioRenderer)
    }

    // MARK: control (called from the main actor)

    func start(isLive: Bool) {
        queue.async {
            self.isLive = isLive
            self.running = true
            self.paused = false
            self.startedAt = Date()
            self.resetDecoding(keepPrograms: false)
        }
    }

    func stop() {
        queue.async {
            self.running = false
            self.flushRenderers()
            self.synchronizer.setRate(0, time: .zero)
            self.demuxer = TransportStreamDemuxer()
        }
    }

    func setPaused(_ p: Bool) {
        queue.async {
            self.paused = p
            guard self.clockStarted else { return }
            self.synchronizer.rate = p ? 0 : 1
        }
    }

    // MARK: URLSessionDataDelegate (delivered on `queue`)

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            onEvent?(.failed("Server answered HTTP \(http.statusCode)"))
            completionHandler(.cancel)
            return
        }
        onEvent?(.connected)
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard running else { return }
        bytesReceived += Int64(data.count)
        for event in demuxer.push(bytes: data) {
            handle(event)
        }
        reportProgress()
        applyBackpressure()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard running else { return }
        if let error {
            if (error as NSError).code == NSURLErrorCancelled { return }
            onEvent?(.failed(error.localizedDescription))
        } else {
            for event in demuxer.flush() { handle(event) }
            onEvent?(.ended)
        }
    }

    // MARK: demux events

    private func handle(_ event: FfiDemuxEvent) {
        switch event {
        case .programFound(_, _, let streams):
            selectStreams(streams)
        case .accessUnit(let pid, let codec, let rawPts, let rawDts, let randomAccess, let data):
            if pid == videoPid {
                handleVideo(codec: codec, pts: rawPts, dts: rawDts, randomAccess: randomAccess, data: data)
            } else if pid == audioPid {
                handleAudio(codec: codec, pts: rawPts, data: data)
            }
        case .discontinuity(let pid):
            if pid == videoPid || pid == audioPid {
                discontinuities += 1
                resetDecoding(keepPrograms: true)
                onEvent?(.buffering)
            }
        case .pcr, .syncLost, .crcError:
            break
        }
    }

    private func selectStreams(_ streams: [FfiStream]) {
        let video = streams.first { $0.codec == .h264 || $0.codec == .h265 }
        let audioPreference: [FfiCodec] = [.aacAdts, .ac3, .eac3, .mpegAudio]
        let audio = audioPreference.lazy.compactMap { pref in streams.first { $0.codec == pref } }.first
        if video?.pid != videoPid || audio?.pid != audioPid {
            resetDecoding(keepPrograms: true)
        }
        videoPid = video?.pid
        audioPid = audio?.pid
        videoCodec = video?.codec ?? .other
        audioCodec = audio?.codec ?? .other
        if video == nil { startupBuffer = 0.5 }
        onEvent?(.streams(video: video.map { "\($0.codec)" }, audio: audio.map { "\($0.codec)" }))
    }

    private func resetDecoding(keepPrograms: Bool) {
        flushRenderers()
        synchronizer.setRate(0, time: .zero)
        clockStarted = false
        firstPts = nil
        lastAudioPts = nil
        lastVideoPts = nil
        pts.reset()
        videoFormat = nil
        audioFormat = nil
        h264Sets = nil
        hevcSets = nil
        adtsHeader = nil
        ac3Info = nil
        mpegInfo = nil
        if keepPrograms {
            demuxer.resetTiming()
        } else {
            demuxer = TransportStreamDemuxer()
            videoPid = nil
            audioPid = nil
            bytesReceived = 0
            discontinuities = 0
        }
    }

    private func flushRenderers() {
        videoRenderer.flush()
        audioRenderer.flush()
    }

    // MARK: video

    private func handleVideo(codec: FfiCodec, pts rawPts: UInt64?, dts rawDts: UInt64?, randomAccess: Bool, data: Data) {
        guard let rawPts else { return }
        let nals = AnnexB.nalUnits(data)
        guard !nals.isEmpty else { return }

        let isKeyframe: Bool
        let payloadNals: [Data]
        switch codec {
        case .h264:
            if let sets = H264ParameterSets.extract(from: nals), sets != h264Sets {
                h264Sets = sets
                videoFormat = Self.makeH264Format(sets)
            }
            isKeyframe = randomAccess || H264ParameterSets.isKeyframe(nals)
            payloadNals = nals.filter { !H264ParameterSets.excludedTypes.contains(AnnexB.h264Type($0)) }
        case .h265:
            if let sets = HEVCParameterSets.extract(from: nals), sets != hevcSets {
                hevcSets = sets
                videoFormat = Self.makeHEVCFormat(sets)
            }
            isKeyframe = randomAccess || HEVCParameterSets.isKeyframe(nals)
            payloadNals = nals.filter { !HEVCParameterSets.excludedTypes.contains(AnnexB.hevcType($0)) }
        default:
            return
        }
        guard let format = videoFormat, !payloadNals.isEmpty else { return }

        let ptsValue = pts.unwrap(rawPts)
        let dtsValue = rawDts.map { pts.unwrap($0) }
        if firstPts == nil { firstPts = dtsValue ?? ptsValue }
        lastVideoPts = ptsValue

        let packed = AnnexB.lengthPrefixed(payloadNals)
        guard let sample = Self.makeSampleBuffer(
            data: packed,
            format: format,
            pts: CMTime(value: ptsValue, timescale: Self.timescale),
            dts: dtsValue.map { CMTime(value: $0, timescale: Self.timescale) },
            duration: .invalid,
            isSync: isKeyframe
        ) else { return }
        videoRenderer.enqueue(sample)
        maybeStartClock()
    }

    // MARK: audio

    private func handleAudio(codec: FfiCodec, pts rawPts: UInt64?, data: Data) {
        let bytes = [UInt8](data)
        var cursor: Int64? = rawPts.map { pts.unwrap($0) } ?? lastAudioPts
        guard cursor != nil else { return }

        func enqueue(_ frameBytes: [UInt8], format: CMAudioFormatDescription, samples: Int, sampleRate: Int) {
            guard let start = cursor else { return }
            let duration = CMTime(value: CMTimeValue(samples) * CMTimeValue(Self.timescale) / CMTimeValue(sampleRate), timescale: Self.timescale)
            if let sample = Self.makeSampleBuffer(
                data: Data(frameBytes),
                format: format,
                pts: CMTime(value: start, timescale: Self.timescale),
                dts: nil,
                duration: duration,
                isSync: true
            ) {
                audioRenderer.enqueue(sample)
            }
            if firstPts == nil { firstPts = start }
            cursor = start + duration.value
            lastAudioPts = cursor
        }

        switch codec {
        case .aacAdts:
            for (header, range) in ADTSHeader.frames(in: bytes) {
                if header != adtsHeader || audioFormat == nil {
                    adtsHeader = header
                    audioFormat = Self.makeAACFormat(header)
                }
                guard let format = audioFormat else { continue }
                enqueue(Array(bytes[range]), format: format, samples: 1024, sampleRate: header.sampleRate)
            }
        case .ac3, .eac3:
            for (info, range) in AC3.frames(in: bytes) {
                if info != ac3Info || audioFormat == nil {
                    ac3Info = info
                    audioFormat = Self.makeAC3Format(info)
                }
                guard let format = audioFormat else { continue }
                enqueue(Array(bytes[range]), format: format, samples: info.samplesPerFrame, sampleRate: info.sampleRate)
            }
        case .mpegAudio:
            for (info, range) in MPEGAudio.frames(in: bytes) {
                if info != mpegInfo || audioFormat == nil {
                    mpegInfo = info
                    audioFormat = Self.makeMPEGAudioFormat(info)
                }
                guard let format = audioFormat else { continue }
                enqueue(Array(bytes[range]), format: format, samples: info.samplesPerFrame, sampleRate: info.sampleRate)
            }
        default:
            return
        }
        maybeStartClock()
    }

    // MARK: clock and buffering

    private var bufferedSeconds: Double {
        guard let first = firstPts else { return 0 }
        let reference = audioPid != nil ? lastAudioPts : lastVideoPts
        guard let last = reference else { return 0 }
        if clockStarted {
            let now = CMTimeGetSeconds(synchronizer.currentTime())
            return max(0, Double(last) / Double(Self.timescale) - now)
        }
        return Double(last - first) / Double(Self.timescale)
    }

    private func maybeStartClock() {
        guard !clockStarted, let first = firstPts else { return }
        let waitedTooLong = Date().timeIntervalSince(startedAt) > 6
        guard bufferedSeconds >= startupBuffer || waitedTooLong else { return }
        clockStarted = true
        let startTime = CMTime(value: first, timescale: Self.timescale)
        synchronizer.setRate(paused ? 0 : 1, time: startTime)
        onEvent?(.playing(Date().timeIntervalSince(startedAt)))
    }

    private var lastProgressReport = Date.distantPast
    private func reportProgress() {
        guard Date().timeIntervalSince(lastProgressReport) > 0.5 else { return }
        lastProgressReport = Date()
        onEvent?(.progress(bytes: bytesReceived, buffered: bufferedSeconds, discontinuities: discontinuities))
        // Grow the startup buffer when the stream keeps stalling.
        if clockStarted, bufferedSeconds < 0.1, isLive {
            startupBuffer = min(4, startupBuffer + 0.5)
        }
    }

    /// For non-live TS (a file served over HTTP) the socket would run far
    /// ahead of playback; pause the transfer when we hold enough.
    private func applyBackpressure() {
        guard !isLive, let task else { return }
        if !suspended, bufferedSeconds > 10 {
            suspended = true
            task.suspend()
        } else if suspended, bufferedSeconds < 5 {
            suspended = false
            task.resume()
        }
    }

    // MARK: CoreMedia helpers

    private static func makeSampleBuffer(
        data: Data, format: CMFormatDescription, pts: CMTime, dts: CMTime?, duration: CMTime, isSync: Bool
    ) -> CMSampleBuffer? {
        var blockBuffer: CMBlockBuffer?
        let count = data.count
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: count,
            blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
            dataLength: count, flags: 0, blockBufferOut: &blockBuffer
        ) == kCMBlockBufferNoErr, let block = blockBuffer else { return nil }

        let copied = data.withUnsafeBytes { raw -> OSStatus in
            guard let base = raw.baseAddress else { return -1 }
            return CMBlockBufferReplaceDataBytes(with: base, blockBuffer: block, offsetIntoDestination: 0, dataLength: count)
        }
        guard copied == kCMBlockBufferNoErr else { return nil }

        var timing = CMSampleTimingInfo(duration: duration, presentationTimeStamp: pts, decodeTimeStamp: dts ?? .invalid)
        var size = count
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
            sampleCount: 1, sampleTimingEntryCount: 1, sampleTimingArray: &timing,
            sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample
        ) == noErr, let sample else { return nil }

        if !isSync,
           let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(
                dict,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_NotSync).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            )
        }
        return sample
    }

    private static func makeH264Format(_ sets: H264ParameterSets) -> CMVideoFormatDescription? {
        var format: CMVideoFormatDescription?
        sets.sps.withUnsafeBytes { spsRaw in
            sets.pps.withUnsafeBytes { ppsRaw in
                guard let sps = spsRaw.bindMemory(to: UInt8.self).baseAddress,
                      let pps = ppsRaw.bindMemory(to: UInt8.self).baseAddress else { return }
                let pointers: [UnsafePointer<UInt8>] = [sps, pps]
                let sizes: [Int] = [sets.sps.count, sets.pps.count]
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: 2,
                    parameterSetPointers: pointers, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, formatDescriptionOut: &format
                )
            }
        }
        return format
    }

    private static func makeHEVCFormat(_ sets: HEVCParameterSets) -> CMVideoFormatDescription? {
        var format: CMVideoFormatDescription?
        sets.vps.withUnsafeBytes { vpsRaw in
            sets.sps.withUnsafeBytes { spsRaw in
                sets.pps.withUnsafeBytes { ppsRaw in
                    guard let vps = vpsRaw.bindMemory(to: UInt8.self).baseAddress,
                          let sps = spsRaw.bindMemory(to: UInt8.self).baseAddress,
                          let pps = ppsRaw.bindMemory(to: UInt8.self).baseAddress else { return }
                    let pointers: [UnsafePointer<UInt8>] = [vps, sps, pps]
                    let sizes: [Int] = [sets.vps.count, sets.sps.count, sets.pps.count]
                    CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        allocator: kCFAllocatorDefault, parameterSetCount: 3,
                        parameterSetPointers: pointers, parameterSetSizes: sizes,
                        nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &format
                    )
                }
            }
        }
        return format
    }

    private static func makeAACFormat(_ header: ADTSHeader) -> CMAudioFormatDescription? {
        var asbd = AudioStreamBasicDescription(
            mSampleRate: Float64(header.sampleRate), mFormatID: kAudioFormatMPEG4AAC, mFormatFlags: 0,
            mBytesPerPacket: 0, mFramesPerPacket: 1024, mBytesPerFrame: 0,
            mChannelsPerFrame: UInt32(header.channels), mBitsPerChannel: 0, mReserved: 0
        )
        let cookie = header.audioSpecificConfig
        var format: CMAudioFormatDescription?
        cookie.withUnsafeBytes { raw in
            CMAudioFormatDescriptionCreate(
                allocator: kCFAllocatorDefault, asbd: &asbd, layoutSize: 0, layout: nil,
                magicCookieSize: cookie.count, magicCookie: raw.baseAddress,
                extensions: nil, formatDescriptionOut: &format
            )
        }
        return format
    }

    private static func makeAC3Format(_ info: AC3.FrameInfo) -> CMAudioFormatDescription? {
        var asbd = AudioStreamBasicDescription(
            mSampleRate: Float64(info.sampleRate),
            mFormatID: info.isEnhanced ? kAudioFormatEnhancedAC3 : kAudioFormatAC3,
            mFormatFlags: 0, mBytesPerPacket: 0, mFramesPerPacket: UInt32(info.samplesPerFrame),
            mBytesPerFrame: 0, mChannelsPerFrame: UInt32(info.channels), mBitsPerChannel: 0, mReserved: 0
        )
        var format: CMAudioFormatDescription?
        CMAudioFormatDescriptionCreate(
            allocator: kCFAllocatorDefault, asbd: &asbd, layoutSize: 0, layout: nil,
            magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &format
        )
        return format
    }

    private static func makeMPEGAudioFormat(_ info: MPEGAudio.FrameInfo) -> CMAudioFormatDescription? {
        let formatID: AudioFormatID
        switch info.layer {
        case 1: formatID = kAudioFormatMPEGLayer1
        case 2: formatID = kAudioFormatMPEGLayer2
        default: formatID = kAudioFormatMPEGLayer3
        }
        var asbd = AudioStreamBasicDescription(
            mSampleRate: Float64(info.sampleRate), mFormatID: formatID, mFormatFlags: 0,
            mBytesPerPacket: 0, mFramesPerPacket: UInt32(info.samplesPerFrame), mBytesPerFrame: 0,
            mChannelsPerFrame: UInt32(info.channels), mBitsPerChannel: 0, mReserved: 0
        )
        var format: CMAudioFormatDescription?
        CMAudioFormatDescriptionCreate(
            allocator: kCFAllocatorDefault, asbd: &asbd, layoutSize: 0, layout: nil,
            magicCookieSize: 0, magicCookie: nil, extensions: nil, formatDescriptionOut: &format
        )
        return format
    }
}
