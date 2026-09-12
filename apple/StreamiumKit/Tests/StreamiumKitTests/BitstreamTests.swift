import XCTest
@testable import StreamiumKit

final class BitstreamTests: XCTestCase {
    func testAnnexBSplitAndRepack() {
        let data = Data([0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0, 0, 0, 1, 0x65, 0xCC, 0xDD])
        let nals = AnnexB.nalUnits(data)
        XCTAssertEqual(nals, [Data([0x67, 0xAA]), Data([0x68, 0xBB]), Data([0x65, 0xCC, 0xDD])])
        XCTAssertEqual(AnnexB.h264Type(nals[0]), 7)
        XCTAssertEqual(AnnexB.h264Type(nals[2]), 5)
        XCTAssertTrue(H264ParameterSets.isKeyframe(nals))
        let sets = H264ParameterSets.extract(from: nals)
        XCTAssertEqual(sets?.sps, Data([0x67, 0xAA]))
        XCTAssertEqual(sets?.pps, Data([0x68, 0xBB]))
        let packed = AnnexB.lengthPrefixed([nals[2]])
        XCTAssertEqual(packed, Data([0, 0, 0, 3, 0x65, 0xCC, 0xDD]))
    }

    func testHEVCTypes() {
        let idr = Data([0x26, 0x01]) // type 19
        XCTAssertEqual(AnnexB.hevcType(idr), 19)
        XCTAssertTrue(HEVCParameterSets.isKeyframe([idr]))
        XCTAssertFalse(HEVCParameterSets.isKeyframe([Data([0x02, 0x01])]))
    }

    func testADTSHeader() {
        // syncword, MPEG-4, no CRC, AAC LC (profile 1), 48 kHz (index 3), 2 channels, frame length 7 + 300
        let length = 307
        let bytes: [UInt8] = [
            0xFF, 0xF1,
            (1 << 6) | (3 << 2) | 0,
            (2 << 6) | UInt8((length >> 11) & 0x03),
            UInt8((length >> 3) & 0xFF),
            UInt8((length & 0x07) << 5) | 0x1F,
            0xFC,
        ] + [UInt8](repeating: 0xAB, count: 300)
        let frames = ADTSHeader.frames(in: bytes + bytes)
        XCTAssertEqual(frames.count, 2)
        let h = frames[0].0
        XCTAssertEqual(h.sampleRate, 48000)
        XCTAssertEqual(h.channels, 2)
        XCTAssertEqual(h.audioObjectType, 2)
        XCTAssertEqual(h.headerLength, 7)
        XCTAssertEqual(frames[0].1, 7..<307)
        XCTAssertEqual(frames[1].1, 314..<614)
        XCTAssertEqual(h.audioSpecificConfig, Data([0x11, 0x90]))
    }

    func testAC3FrameSize() {
        // AC-3, 48 kHz (fscod 0), frmsizecod 28 → 384 kbps → 768 words → 1536 bytes
        var bytes: [UInt8] = [0x0B, 0x77, 0x00, 0x00, (0 << 6) | 28, 0x40, 0x00]
        bytes += [UInt8](repeating: 0, count: 1536 - bytes.count)
        let f = AC3.parse(bytes, at: 0)
        XCTAssertEqual(f?.frameLength, 1536)
        XCTAssertEqual(f?.sampleRate, 48000)
        XCTAssertEqual(f?.isEnhanced, false)
        XCTAssertEqual(AC3.frames(in: bytes).count, 1)
    }

    func testMPEGAudioLayerII() {
        // MPEG-1 Layer II, 192 kbps (index 10), 48 kHz (index 1), no padding, stereo
        let header: [UInt8] = [0xFF, 0xFD, (10 << 4) | (1 << 2), 0x00]
        let f = MPEGAudio.parse(header + [UInt8](repeating: 0, count: 600), at: 0)
        XCTAssertEqual(f?.layer, 2)
        XCTAssertEqual(f?.sampleRate, 48000)
        XCTAssertEqual(f?.frameLength, 576)
        XCTAssertEqual(f?.samplesPerFrame, 1152)
        XCTAssertEqual(f?.channels, 2)
    }

    func testPtsUnwrapAcrossWrap() {
        var u = PtsUnwrapper()
        let max: UInt64 = (1 << 33) - 1
        XCTAssertEqual(u.unwrap(max - 100), Int64(max - 100))
        XCTAssertEqual(u.unwrap(50), Int64(max) + 51)
        XCTAssertEqual(u.unwrap(100), Int64(max) + 101)
    }

    func testProbeSniff() {
        var ts = [UInt8](repeating: 0, count: 188 * 3)
        ts[0] = 0x47; ts[188] = 0x47; ts[376] = 0x47
        XCTAssertEqual(StreamProbe.sniff(Data(ts)), .mpegTs)
        XCTAssertEqual(StreamProbe.sniff(Data("#EXTM3U\n#EXT-X-VERSION:3".utf8)), .hls)
        XCTAssertEqual(StreamProbe.sniff(Data([0, 0, 0, 0x18] + Array("ftypisom".utf8))), .progressive)
        XCTAssertEqual(StreamProbe.sniff(Data()), .unknown)
    }
}
