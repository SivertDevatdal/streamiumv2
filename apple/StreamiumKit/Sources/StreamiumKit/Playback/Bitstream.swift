import Foundation

// Pure-Swift bitstream helpers used by TransportStreamEngine. They have no
// AVFoundation dependency so they can be unit-tested on any Apple platform.

/// Annex B (start-code delimited) NAL unit handling for H.264 and HEVC.
public enum AnnexB {
    /// Split an Annex B byte stream into NAL unit payloads (start codes removed).
    public static func nalUnits(_ data: Data) -> [Data] {
        let bytes = [UInt8](data)
        var starts: [Int] = []
        var i = 0
        while i + 3 <= bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                starts.append(i + 3)
                i += 3
            } else {
                i += 1
            }
        }
        var out: [Data] = []
        out.reserveCapacity(starts.count)
        for (k, s) in starts.enumerated() {
            var e = (k + 1 < starts.count) ? starts[k + 1] - 3 : bytes.count
            while e > s, bytes[e - 1] == 0 { e -= 1 } // zero byte of a 4-byte start code
            if e > s { out.append(Data(bytes[s..<e])) }
        }
        return out
    }

    /// Re-pack NAL units with 4-byte big-endian length prefixes (the format
    /// VideoToolbox expects for `avcC` / `hvcC` streams).
    public static func lengthPrefixed(_ nals: [Data]) -> Data {
        var out = Data()
        out.reserveCapacity(nals.reduce(0) { $0 + $1.count + 4 })
        for nal in nals {
            let n = UInt32(nal.count).bigEndian
            withUnsafeBytes(of: n) { out.append(contentsOf: $0) }
            out.append(nal)
        }
        return out
    }

    public static func h264Type(_ nal: Data) -> UInt8 { nal.first.map { $0 & 0x1F } ?? 0 }
    public static func hevcType(_ nal: Data) -> UInt8 { nal.first.map { ($0 >> 1) & 0x3F } ?? 0 }
}

/// Parameter sets needed to build a CMVideoFormatDescription.
public struct H264ParameterSets: Equatable {
    public var sps: Data
    public var pps: Data

    /// Extract from an access unit. Returns nil when either set is missing.
    public static func extract(from nals: [Data]) -> H264ParameterSets? {
        var sps: Data?
        var pps: Data?
        for nal in nals {
            switch AnnexB.h264Type(nal) {
            case 7: if sps == nil { sps = nal }
            case 8: if pps == nil { pps = nal }
            default: break
            }
        }
        guard let s = sps, let p = pps else { return nil }
        return H264ParameterSets(sps: s, pps: p)
    }

    public static func isKeyframe(_ nals: [Data]) -> Bool {
        nals.contains { AnnexB.h264Type($0) == 5 }
    }

    /// NAL types that must not be sent to the decoder as sample data.
    public static let excludedTypes: Set<UInt8> = [7, 8, 9] // SPS, PPS, AUD
}

public struct HEVCParameterSets: Equatable {
    public var vps: Data
    public var sps: Data
    public var pps: Data

    public static func extract(from nals: [Data]) -> HEVCParameterSets? {
        var vps: Data?
        var sps: Data?
        var pps: Data?
        for nal in nals {
            switch AnnexB.hevcType(nal) {
            case 32: if vps == nil { vps = nal }
            case 33: if sps == nil { sps = nal }
            case 34: if pps == nil { pps = nal }
            default: break
            }
        }
        guard let v = vps, let s = sps, let p = pps else { return nil }
        return HEVCParameterSets(vps: v, sps: s, pps: p)
    }

    public static func isKeyframe(_ nals: [Data]) -> Bool {
        nals.contains { (16...21).contains(AnnexB.hevcType($0)) }
    }

    public static let excludedTypes: Set<UInt8> = [32, 33, 34, 35] // VPS, SPS, PPS, AUD
}

/// ADTS header for AAC in transport streams.
public struct ADTSHeader: Equatable {
    public var audioObjectType: UInt8 // 1 = Main, 2 = LC, 3 = SSR, 4 = LTP (profile + 1)
    public var sampleRateIndex: UInt8
    public var sampleRate: Int
    public var channelConfiguration: UInt8
    public var headerLength: Int // 7, or 9 with CRC
    public var frameLength: Int  // including the header

    public var channels: Int { channelConfiguration == 7 ? 8 : Int(channelConfiguration) }

    public static let sampleRates = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]

    /// Parse the header at `offset`. Returns nil when there is no valid syncword.
    public static func parse(_ bytes: [UInt8], at offset: Int) -> ADTSHeader? {
        guard offset + 7 <= bytes.count else { return nil }
        guard bytes[offset] == 0xFF, bytes[offset + 1] & 0xF6 == 0xF0 else { return nil }
        let protectionAbsent = bytes[offset + 1] & 0x01 == 1
        let profile = (bytes[offset + 2] >> 6) & 0x03
        let srIndex = (bytes[offset + 2] >> 2) & 0x0F
        guard Int(srIndex) < sampleRates.count else { return nil }
        let channelConfig = ((bytes[offset + 2] & 0x01) << 2) | ((bytes[offset + 3] >> 6) & 0x03)
        let frameLength = (Int(bytes[offset + 3] & 0x03) << 11) | (Int(bytes[offset + 4]) << 3) | (Int(bytes[offset + 5]) >> 5)
        let headerLength = protectionAbsent ? 7 : 9
        guard frameLength >= headerLength else { return nil }
        return ADTSHeader(
            audioObjectType: profile + 1,
            sampleRateIndex: srIndex,
            sampleRate: sampleRates[Int(srIndex)],
            channelConfiguration: channelConfig,
            headerLength: headerLength,
            frameLength: frameLength
        )
    }

    /// Two-byte AudioSpecificConfig used as the decoder's magic cookie.
    public var audioSpecificConfig: Data {
        let objectType = UInt16(audioObjectType)
        let bits: UInt16 = (objectType << 11) | (UInt16(sampleRateIndex) << 7) | (UInt16(channelConfiguration) << 3)
        return Data([UInt8(bits >> 8), UInt8(bits & 0xFF)])
    }

    /// Split a PES payload into `(header, payloadRange)` pairs, skipping junk
    /// between frames.
    ///
    /// The range covers the raw AAC payload **without** the ADTS header,
    /// because VideoToolbox and CoreAudio want bare AAC frames described by
    /// the `audioSpecificConfig` magic cookie. AC-3 and MPEG audio differ:
    /// their ranges include the sync frame header, which the decoder parses
    /// itself.
    public static func frames(in bytes: [UInt8]) -> [(ADTSHeader, Range<Int>)] {
        var out: [(ADTSHeader, Range<Int>)] = []
        var i = 0
        while i + 7 <= bytes.count {
            guard let h = parse(bytes, at: i) else { i += 1; continue }
            let end = i + h.frameLength
            guard end <= bytes.count else { break }
            out.append((h, (i + h.headerLength)..<end))
            i = end
        }
        return out
    }
}

/// Reads big-endian bit fields, which is how AC-3's bit stream information
/// field is laid out. Returns nil rather than trapping when it runs out.
struct BitReader {
    private let bytes: [UInt8]
    private var position: Int

    init(_ bytes: [UInt8], byteOffset: Int) {
        self.bytes = bytes
        self.position = byteOffset * 8
    }

    mutating func read(_ count: Int) -> Int? {
        var value = 0
        for _ in 0..<count {
            let byteIndex = position >> 3
            guard byteIndex < bytes.count else { return nil }
            let bit = (bytes[byteIndex] >> (7 - UInt8(position & 7))) & 1
            value = (value << 1) | Int(bit)
            position += 1
        }
        return value
    }

    @discardableResult
    mutating func skip(_ count: Int) -> Bool { read(count) != nil }
}

/// AC-3 and E-AC-3 sync frame parsing (sizes only; CoreAudio decodes the rest).
public enum AC3 {
    public struct FrameInfo: Equatable {
        public var isEnhanced: Bool
        public var sampleRate: Int
        public var frameLength: Int
        public var samplesPerFrame: Int
        public var channels: Int
    }

    static let bitrates = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640]
    static let frameWords441 = [
        69, 70, 87, 88, 104, 105, 121, 122, 139, 140, 174, 175, 208, 209, 243, 244, 278, 279, 348, 349,
        417, 418, 487, 488, 557, 558, 696, 697, 835, 836, 975, 976, 1114, 1115, 1253, 1254, 1393, 1394,
    ]
    static let acmodChannels = [2, 1, 2, 3, 3, 4, 4, 5]

    public static func parse(_ bytes: [UInt8], at i: Int) -> FrameInfo? {
        guard i + 7 <= bytes.count, bytes[i] == 0x0B, bytes[i + 1] == 0x77 else { return nil }
        let bsid = bytes[i + 5] >> 3
        if bsid <= 10 {
            // AC-3
            let fscod = Int(bytes[i + 4] >> 6)
            let frmsizecod = Int(bytes[i + 4] & 0x3F)
            guard fscod < 3, frmsizecod < 38 else { return nil }
            let kbps = bitrates[frmsizecod >> 1]
            let words: Int
            let sampleRate: Int
            switch fscod {
            case 0: sampleRate = 48000; words = kbps * 2
            case 1: sampleRate = 44100; words = frameWords441[frmsizecod]
            default: sampleRate = 32000; words = kbps * 3
            }
            // The bit stream information field begins at byte 5 with bsid and
            // bsmod. acmod is followed by mix-level fields that are only
            // present for certain channel modes, so lfeon has no fixed
            // position and the field must be walked bit by bit.
            var reader = BitReader(bytes, byteOffset: i + 5)
            guard reader.skip(5), reader.skip(3), let acmod = reader.read(3) else { return nil }
            if acmod & 0x01 != 0, acmod != 0x01 {
                guard reader.skip(2) else { return nil } // cmixlev
            }
            if acmod & 0x04 != 0 {
                guard reader.skip(2) else { return nil } // surmixlev
            }
            if acmod == 0x02 {
                guard reader.skip(2) else { return nil } // dsurmod
            }
            guard let lfeon = reader.read(1) else { return nil }
            return FrameInfo(
                isEnhanced: false,
                sampleRate: sampleRate,
                frameLength: words * 2,
                samplesPerFrame: 1536,
                channels: acmodChannels[acmod] + lfeon
            )
        } else if bsid <= 16 {
            // E-AC-3
            let frmsiz = (Int(bytes[i + 2] & 0x07) << 8) | Int(bytes[i + 3])
            let fscod = Int(bytes[i + 4] >> 6)
            var sampleRate: Int
            var numblks: Int
            if fscod == 3 {
                let fscod2 = Int((bytes[i + 4] >> 4) & 0x03)
                sampleRate = [24000, 22050, 16000, 0][fscod2]
                numblks = 6
            } else {
                sampleRate = [48000, 44100, 32000][fscod]
                let numblkscod = Int((bytes[i + 4] >> 4) & 0x03)
                numblks = [1, 2, 3, 6][numblkscod]
            }
            guard sampleRate > 0 else { return nil }
            // E-AC-3 puts acmod and lfeon at fixed positions in byte 4.
            let acmod = Int((bytes[i + 4] >> 1) & 0x07)
            let lfe = bytes[i + 4] & 0x01 == 1
            return FrameInfo(
                isEnhanced: true,
                sampleRate: sampleRate,
                frameLength: (frmsiz + 1) * 2,
                samplesPerFrame: numblks * 256,
                channels: acmodChannels[acmod] + (lfe ? 1 : 0)
            )
        }
        return nil
    }

    /// Split a payload into `(info, frameRange)` pairs. Unlike AAC, the range
    /// includes the sync frame header: CoreAudio decodes complete AC-3 frames.
    public static func frames(in bytes: [UInt8]) -> [(FrameInfo, Range<Int>)] {
        var out: [(FrameInfo, Range<Int>)] = []
        var i = 0
        while i + 7 <= bytes.count {
            guard let f = parse(bytes, at: i) else { i += 1; continue }
            let end = i + f.frameLength
            guard end <= bytes.count else { break }
            out.append((f, i..<end))
            i = end
        }
        return out
    }
}

/// MPEG-1/2 audio (Layer I/II/III) frame parsing.
public enum MPEGAudio {
    public struct FrameInfo: Equatable {
        public var layer: Int // 1, 2, 3
        public var sampleRate: Int
        public var channels: Int
        public var frameLength: Int
        public var samplesPerFrame: Int
    }

    static let v1Bitrates: [[Int]] = [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ]
    static let v2Bitrates: [[Int]] = [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ]

    public static func parse(_ bytes: [UInt8], at i: Int) -> FrameInfo? {
        guard i + 4 <= bytes.count, bytes[i] == 0xFF, bytes[i + 1] & 0xE0 == 0xE0 else { return nil }
        let versionBits = (bytes[i + 1] >> 3) & 0x03 // 0 = 2.5, 2 = 2, 3 = 1
        let layerBits = (bytes[i + 1] >> 1) & 0x03   // 1 = III, 2 = II, 3 = I
        let bitrateIndex = Int(bytes[i + 2] >> 4)
        let srIndex = Int((bytes[i + 2] >> 2) & 0x03)
        let padding = Int((bytes[i + 2] >> 1) & 0x01)
        let channelMode = (bytes[i + 3] >> 6) & 0x03
        guard versionBits != 1, layerBits != 0, bitrateIndex != 0, bitrateIndex != 15, srIndex != 3 else { return nil }
        let layer = 4 - Int(layerBits)
        let isV1 = versionBits == 3
        let baseRates = isV1 ? [44100, 48000, 32000] : (versionBits == 2 ? [22050, 24000, 16000] : [11025, 12000, 8000])
        let sampleRate = baseRates[srIndex]
        let kbps = (isV1 ? v1Bitrates : v2Bitrates)[layer - 1][bitrateIndex]
        let bitrate = kbps * 1000
        let frameLength: Int
        let samples: Int
        switch layer {
        case 1:
            frameLength = (12 * bitrate / sampleRate + padding) * 4
            samples = 384
        case 2:
            frameLength = 144 * bitrate / sampleRate + padding
            samples = 1152
        default:
            frameLength = (isV1 ? 144 : 72) * bitrate / sampleRate + padding
            samples = isV1 ? 1152 : 576
        }
        guard frameLength > 4 else { return nil }
        return FrameInfo(
            layer: layer,
            sampleRate: sampleRate,
            channels: channelMode == 3 ? 1 : 2,
            frameLength: frameLength,
            samplesPerFrame: samples
        )
    }

    /// Split a payload into `(info, frameRange)` pairs. The range includes the
    /// frame header, which the decoder parses itself.
    public static func frames(in bytes: [UInt8]) -> [(FrameInfo, Range<Int>)] {
        var out: [(FrameInfo, Range<Int>)] = []
        var i = 0
        while i + 4 <= bytes.count {
            guard let f = parse(bytes, at: i) else { i += 1; continue }
            let end = i + f.frameLength
            guard end <= bytes.count else { break }
            out.append((f, i..<end))
            i = end
        }
        return out
    }
}

/// Extends 33-bit MPEG timestamps to a monotonic 64-bit timeline.
public struct PtsUnwrapper {
    private var last: Int64?
    private var offset: Int64 = 0
    private static let wrap: Int64 = 1 << 33
    private static let half: Int64 = 1 << 32

    public init() {}

    public mutating func unwrap(_ raw: UInt64) -> Int64 {
        var value = Int64(raw & 0x1_FFFF_FFFF) + offset
        if let last {
            if value - last < -Self.half {
                offset += Self.wrap
                value += Self.wrap
            } else if value - last > Self.half {
                offset -= Self.wrap
                value -= Self.wrap
            }
        }
        last = value
        return value
    }

    public mutating func reset() {
        last = nil
        offset = 0
    }
}
