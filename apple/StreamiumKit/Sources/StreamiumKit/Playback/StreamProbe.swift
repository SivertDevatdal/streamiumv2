import Foundation
import StreamiumCore

/// Decides which engine should open a URL. Uses the extension when it is
/// conclusive, otherwise downloads the first couple of kilobytes and sniffs.
public enum StreamProbe {
    public static func probe(_ item: PlaybackItem, session: URLSession = .shared) async -> FfiStreamFormat {
        if item.format != .unknown { return item.format }
        guard item.url.scheme?.lowercased().hasPrefix("http") == true else { return .progressive }

        var request = URLRequest(url: item.url)
        request.timeoutInterval = 6
        request.setValue("bytes=0-4095", forHTTPHeaderField: "Range")
        for (k, v) in item.httpHeaders { request.setValue(v, forHTTPHeaderField: k) }

        do {
            let (bytes, response) = try await session.bytes(for: request)
            if let http = response as? HTTPURLResponse,
               let type = http.value(forHTTPHeaderField: "Content-Type")?.lowercased() {
                if type.contains("mpegurl") { return .hls }
                if type.contains("mp2t") { return .mpegTs }
                if type.contains("dash+xml") { return .dash }
            }
            var buffer = Data()
            buffer.reserveCapacity(2048)
            for try await byte in bytes {
                buffer.append(byte)
                if buffer.count >= 2048 { break }
            }
            return sniff(buffer)
        } catch {
            return .unknown
        }
    }

    /// Classify a stream by its first bytes.
    public static func sniff(_ data: Data) -> FfiStreamFormat {
        let bytes = [UInt8](data)
        guard !bytes.isEmpty else { return .unknown }

        // Transport stream: 0x47 sync bytes 188 apart, at any alignment.
        for offset in 0..<min(188, bytes.count) where bytes[offset] == 0x47 {
            var ok = true
            var i = offset + 188
            var checks = 0
            while i < bytes.count, checks < 3 {
                if bytes[i] != 0x47 { ok = false; break }
                i += 188
                checks += 1
            }
            if ok && (checks >= 1 || bytes.count < offset + 188 * 2) { return .mpegTs }
        }
        if let text = String(bytes: bytes.prefix(16), encoding: .utf8),
           text.trimmingCharacters(in: .whitespacesAndNewlines).uppercased().hasPrefix("#EXTM3U") {
            return .hls
        }
        if bytes.count >= 8, Array(bytes[4..<8]) == Array("ftyp".utf8) { return .progressive }
        if bytes.starts(with: [0x1A, 0x45, 0xDF, 0xA3]) { return .progressive } // Matroska/WebM
        if bytes.starts(with: Array("ID3".utf8)) || bytes.starts(with: Array("RIFF".utf8))
            || bytes.starts(with: Array("fLaC".utf8)) || bytes.starts(with: Array("OggS".utf8)) {
            return .progressive
        }
        if bytes.count >= 2, bytes[0] == 0xFF, bytes[1] & 0xE0 == 0xE0 { return .progressive } // MPEG audio
        if let text = String(bytes: bytes.prefix(64), encoding: .utf8), text.contains("<MPD") { return .dash }
        return .progressive
    }
}

/// Picks and owns the engine for the current item. Views observe this.
@MainActor
public final class PlaybackController: ObservableObject {
    @Published public private(set) var state: PlaybackState = .idle
    @Published public private(set) var engine: PlayerEngine?
    @Published public private(set) var item: PlaybackItem?
    @Published public private(set) var resolvedFormat: FfiStreamFormat = .unknown

    private var generation = 0

    public init() {}

    public func play(_ item: PlaybackItem) {
        generation += 1
        let gen = generation
        engine?.stop()
        engine = nil
        self.item = item
        state = .connecting

        Task { [weak self] in
            let format = await StreamProbe.probe(item)
            guard let self, self.generation == gen else { return }
            self.resolvedFormat = format
            switch format {
            case .rtsp, .udp, .dash:
                self.state = .failed("This stream type (\(format)) needs the fallback engine, which is not built yet.")
                return
            default:
                break
            }
            let engine: PlayerEngine
            if format == .mpegTs {
                engine = TransportStreamEngine()
            } else {
                engine = AVPlayerEngine()
            }
            engine.onStateChange = { [weak self] s in
                guard let self, self.generation == gen else { return }
                self.state = s
            }
            self.engine = engine
            var resolved = item
            resolved.format = format
            engine.load(resolved)
            engine.play()
        }
    }

    public func togglePlayPause() {
        guard let engine else { return }
        if state == .playing { engine.pause() } else { engine.play() }
    }

    public func stop() {
        generation += 1
        engine?.stop()
        engine = nil
        item = nil
        state = .idle
    }
}
