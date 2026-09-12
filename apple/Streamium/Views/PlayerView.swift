import SwiftUI
import StreamiumKit
import QuartzCore

struct PlayerView: View {
    @EnvironmentObject private var playback: PlaybackController
    @State private var showStats = false
    @State private var controlsVisible = true

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            if let engine = playback.engine {
                PlayerLayerView(layer: engine.layer).ignoresSafeArea()
            }
            overlay
        }
        .onTapGesture { withAnimation { controlsVisible.toggle() } }
        .onLongPressGesture { showStats.toggle() }
        #if os(iOS)
        .statusBarHidden(!controlsVisible)
        .persistentSystemOverlays(.hidden)
        #endif
    }

    @ViewBuilder
    private var overlay: some View {
        VStack {
            HStack {
                Button { playback.stop() } label: { Image(systemName: "xmark.circle.fill").font(.title) }
                Spacer()
                Text(playback.item?.title ?? "").font(.headline).lineLimit(1)
                Spacer()
                Button { showStats.toggle() } label: { Image(systemName: "waveform.path.ecg") }
            }
            .padding()
            .opacity(controlsVisible ? 1 : 0)

            Spacer()

            switch playback.state {
            case .connecting, .buffering:
                ProgressView().controlSize(.large).tint(.white)
            case .failed(let message):
                VStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle").font(.largeTitle)
                    Text(message).multilineTextAlignment(.center)
                }
                .padding()
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
            default:
                EmptyView()
            }

            Spacer()

            HStack(spacing: 32) {
                Button { playback.togglePlayPause() } label: {
                    Image(systemName: playback.state == .playing ? "pause.fill" : "play.fill").font(.largeTitle)
                }
            }
            .padding(.bottom, 24)
            .opacity(controlsVisible ? 1 : 0)
        }
        .foregroundStyle(.white)
        .overlay(alignment: .topTrailing) {
            if showStats, let engine = playback.engine {
                StatsOverlay(stats: engine.stats, state: playback.state, format: "\(playback.resolvedFormat)")
                    .padding(.top, 60)
                    .padding(.trailing)
            }
        }
    }
}

struct StatsOverlay: View {
    let stats: PlaybackStats
    let state: PlaybackState
    let format: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("format: \(format)")
            Text("state: \(String(describing: state))")
            Text("first frame: \(stats.timeToFirstFrame.map { String(format: "%.2f s", $0) } ?? "–")")
            Text("buffer: \(String(format: "%.1f s", stats.bufferedSeconds))")
            Text("rebuffers: \(stats.rebuffers)  discontinuities: \(stats.discontinuities)")
            Text("video: \(stats.videoCodec ?? "–")  audio: \(stats.audioCodec ?? "–")")
            Text("received: \(ByteCountFormatter.string(fromByteCount: stats.bytesReceived, countStyle: .binary))")
        }
        .font(.caption.monospaced())
        .padding(8)
        .background(.black.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
    }
}

/// Hosts an engine's CALayer inside SwiftUI on both UIKit and AppKit.
#if os(macOS)
import AppKit

struct PlayerLayerView: NSViewRepresentable {
    let layer: CALayer

    func makeNSView(context: Context) -> LayerHostView { LayerHostView() }
    func updateNSView(_ view: LayerHostView, context: Context) { view.install(layer) }

    final class LayerHostView: NSView {
        private weak var installed: CALayer?
        override init(frame: NSRect) {
            super.init(frame: frame)
            wantsLayer = true
            layer?.backgroundColor = NSColor.black.cgColor
        }
        required init?(coder: NSCoder) { fatalError() }
        func install(_ l: CALayer) {
            guard installed !== l else { return }
            installed?.removeFromSuperlayer()
            l.frame = bounds
            layer?.addSublayer(l)
            installed = l
        }
        override func layout() {
            super.layout()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            installed?.frame = bounds
            CATransaction.commit()
        }
    }
}
#else
import UIKit

struct PlayerLayerView: UIViewRepresentable {
    let layer: CALayer

    func makeUIView(context: Context) -> LayerHostView { LayerHostView() }
    func updateUIView(_ view: LayerHostView, context: Context) { view.install(layer) }

    final class LayerHostView: UIView {
        private weak var installed: CALayer?
        override init(frame: CGRect) {
            super.init(frame: frame)
            backgroundColor = .black
        }
        required init?(coder: NSCoder) { fatalError() }
        func install(_ l: CALayer) {
            guard installed !== l else { return }
            installed?.removeFromSuperlayer()
            l.frame = bounds
            layer.addSublayer(l)
            installed = l
        }
        override func layoutSubviews() {
            super.layoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            installed?.frame = bounds
            CATransaction.commit()
        }
    }
}
#endif
