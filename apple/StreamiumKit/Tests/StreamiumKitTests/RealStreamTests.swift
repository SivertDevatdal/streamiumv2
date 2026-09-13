import XCTest
@testable import StreamiumKit

/// Checks the bitstream parsers against elementary streams produced by a real
/// encoder, rather than against hand-written byte arrays. ffmpeg demuxes the
/// fixtures itself, so these tests are independent of our own demuxer.
///
/// Generate the fixtures with `scripts/make-test-streams.sh`. When they are
/// absent every test here skips, so `swift test` still works without ffmpeg.
final class RealStreamTests: XCTestCase {

    // MARK: fixtures

    private static let fixtureDirectory: URL = {
        if let override = ProcessInfo.processInfo.environment["STREAMIUM_FIXTURES"] {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        // .../apple/StreamiumKit/Tests/StreamiumKitTests/RealStreamTests.swift
        var root = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { root.deleteLastPathComponent() }
        return root.appendingPathComponent("crates/streamium-mpegts/tests/fixtures", isDirectory: true)
    }()

    private func fixture(_ name: String) throws -> [UInt8] {
        let url = Self.fixtureDirectory.appendingPathComponent(name)
        guard let data = try? Data(contentsOf: url) else {
            throw XCTSkip("missing \(url.path); run scripts/make-test-streams.sh")
        }
        return [UInt8](data)
    }

    // MARK: AAC

    func testADTSFramingMatchesTheEncoder() throws {
        let bytes = try fixture("audio_aac.adts")
        let frames = ADTSHeader.frames(in: bytes)

        XCTAssertEqual(frames.count, 142, "ffmpeg reports 142 AAC frames")
        XCTAssertEqual(Set(frames.map { $0.0.sampleRate }), [48000])
        XCTAssertEqual(Set(frames.map { $0.0.channels }), [2])
        XCTAssertEqual(Set(frames.map { $0.0.audioObjectType }), [2], "AAC LC")
        XCTAssertEqual(Set(frames.map { $0.0.headerLength }), [7], "no CRC in these frames")

        // The frames must tile the file exactly: a gap means a dropped frame,
        // an overlap means a mis-parsed length.
        XCTAssertEqual(frames.first?.1.lowerBound, 7)
        XCTAssertEqual(frames.last?.1.upperBound, bytes.count)
        for (previous, next) in zip(frames, frames.dropFirst()) {
            XCTAssertEqual(previous.1.upperBound + next.0.headerLength, next.1.lowerBound)
        }

        // The magic cookie handed to CoreAudio: AAC LC, 48 kHz, stereo.
        XCTAssertEqual(frames[0].0.audioSpecificConfig, Data([0x11, 0x90]))
    }

    // MARK: AC-3

    func testAC3StereoFraming() throws {
        let bytes = try fixture("audio_ac3.ac3")
        let frames = AC3.frames(in: bytes)

        XCTAssertEqual(frames.count, 94, "ffmpeg reports 94 AC-3 frames")
        XCTAssertEqual(Set(frames.map { $0.0.sampleRate }), [48000])
        XCTAssertEqual(Set(frames.map { $0.0.channels }), [2])
        XCTAssertEqual(Set(frames.map { $0.0.samplesPerFrame }), [1536])
        XCTAssertEqual(Set(frames.map { $0.0.frameLength }), [768], "192 kbps at 48 kHz")
        XCTAssertEqual(Set(frames.map { $0.0.isEnhanced }), [false])
        XCTAssertEqual(frames.last?.1.upperBound, bytes.count, "frames must tile the file")
    }

    /// The LFE flag sits after mix-level fields that are only present for some
    /// channel modes, so its bit position moves. Getting this wrong silently
    /// undercounts channels on the surround content IPTV movie channels carry.
    func testAC3SurroundReportsSixChannels() throws {
        let bytes = try fixture("audio_ac3_51.ac3")
        let frames = AC3.frames(in: bytes)

        XCTAssertEqual(frames.count, 63)
        XCTAssertEqual(Set(frames.map { $0.0.channels }), [6], "5.1 is five channels plus LFE")
        XCTAssertEqual(Set(frames.map { $0.0.frameLength }), [1792], "448 kbps at 48 kHz")
        XCTAssertEqual(frames.last?.1.upperBound, bytes.count)
    }

    func testAC3MonoReportsOneChannel() throws {
        let bytes = try fixture("audio_ac3_mono.ac3")
        let frames = AC3.frames(in: bytes)

        XCTAssertFalse(frames.isEmpty)
        XCTAssertEqual(Set(frames.map { $0.0.channels }), [1])
        XCTAssertEqual(frames.last?.1.upperBound, bytes.count)
    }

    func testTruncatedAC3HeaderIsRejectedNotCrashed() throws {
        let bytes = try fixture("audio_ac3.ac3")
        for length in 0..<16 {
            _ = AC3.parse(Array(bytes.prefix(length)), at: 0)
            _ = AC3.frames(in: Array(bytes.prefix(length)))
        }
    }

    // MARK: MPEG audio

    func testMPEGAudioLayerIIFraming() throws {
        let bytes = try fixture("audio_mp2.mp2")
        let frames = MPEGAudio.frames(in: bytes)

        XCTAssertEqual(frames.count, 125, "ffmpeg reports 125 MP2 frames")
        XCTAssertEqual(Set(frames.map { $0.0.layer }), [2])
        XCTAssertEqual(Set(frames.map { $0.0.sampleRate }), [48000])
        XCTAssertEqual(Set(frames.map { $0.0.channels }), [2])
        XCTAssertEqual(Set(frames.map { $0.0.samplesPerFrame }), [1152])
        XCTAssertEqual(frames.last?.1.upperBound, bytes.count, "frames must tile the file")
    }

    // MARK: H.264

    func testH264ParameterSetsAndKeyframes() throws {
        let bytes = try fixture("video_h264.h264")
        let nals = AnnexB.nalUnits(Data(bytes))

        XCTAssertFalse(nals.isEmpty)
        XCTAssertTrue(nals.allSatisfy { !$0.isEmpty }, "empty NAL units must never be produced")

        let types = nals.map { AnnexB.h264Type($0) }
        XCTAssertEqual(types.filter { $0 == 5 }.count, 3, "one IDR per second of content")
        XCTAssertEqual(types.filter { $0 == 1 || $0 == 5 }.count, 75, "75 coded slices")
        XCTAssertTrue(types.contains(7), "SPS")
        XCTAssertTrue(types.contains(8), "PPS")

        let sets = try XCTUnwrap(H264ParameterSets.extract(from: nals))
        XCTAssertEqual(AnnexB.h264Type(sets.sps), 7)
        XCTAssertEqual(AnnexB.h264Type(sets.pps), 8)
        XCTAssertTrue(H264ParameterSets.isKeyframe(nals))

        // Length prefixing is what VideoToolbox consumes: four bytes per unit.
        let payload = nals.filter { !H264ParameterSets.excludedTypes.contains(AnnexB.h264Type($0)) }
        let packed = AnnexB.lengthPrefixed(payload)
        XCTAssertEqual(packed.count, payload.reduce(0) { $0 + $1.count + 4 })

        // The prefixes must describe the units they precede.
        var offset = 0
        var recovered = 0
        while offset + 4 <= packed.count {
            let length = packed[offset..<(offset + 4)].reduce(0) { Int($0) << 8 | Int($1) }
            offset += 4 + length
            recovered += 1
        }
        XCTAssertEqual(offset, packed.count, "length prefixes must tile the buffer")
        XCTAssertEqual(recovered, payload.count)
    }

    // MARK: HEVC

    func testHEVCParameterSetsAndKeyframes() throws {
        let bytes = try fixture("video_hevc.hevc")
        let nals = AnnexB.nalUnits(Data(bytes))

        let types = nals.map { AnnexB.hevcType($0) }
        XCTAssertEqual(types.filter { (16...21).contains($0) }.count, 3, "one IRAP per second")
        XCTAssertEqual(types.filter { $0 <= 21 }.count, 75, "75 coded slices")
        XCTAssertTrue(types.contains(32), "VPS")
        XCTAssertTrue(types.contains(33), "SPS")
        XCTAssertTrue(types.contains(34), "PPS")

        let sets = try XCTUnwrap(HEVCParameterSets.extract(from: nals))
        XCTAssertEqual(AnnexB.hevcType(sets.vps), 32)
        XCTAssertEqual(AnnexB.hevcType(sets.sps), 33)
        XCTAssertEqual(AnnexB.hevcType(sets.pps), 34)
        XCTAssertTrue(HEVCParameterSets.isKeyframe(nals))
    }

    // MARK: probe

    func testProbeRecognisesRealFiles() throws {
        let ts = try fixture("h264_aac.ts")
        XCTAssertEqual(StreamProbe.sniff(Data(ts.prefix(2048))), .mpegTs)
        // Probing must not depend on the stream starting at a packet boundary.
        XCTAssertEqual(StreamProbe.sniff(Data(ts[37..<2085])), .mpegTs)
    }
}
