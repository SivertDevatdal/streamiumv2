import SwiftUI
import StreamiumKit

enum AppSection: String, CaseIterable, Identifiable {
    case live = "Live TV"
    case guide = "Guide"
    case onDemand = "On Demand"
    case library = "Library"
    case sources = "Sources"

    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .live: return "tv"
        case .guide: return "list.bullet.rectangle"
        case .onDemand: return "film"
        case .library: return "folder"
        case .sources: return "server.rack"
        }
    }
}

struct RootView: View {
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var playback: PlaybackController
    @State private var section: AppSection?

    /// Somewhere to start: the channel list is useless until a source exists,
    /// so a first run opens on Sources instead.
    private var landingSection: AppSection {
        library.sources.isEmpty ? .sources : .live
    }

    var body: some View {
        #if os(macOS)
        NavigationSplitView {
            List(AppSection.allCases, selection: $section) { s in
                Label(s.rawValue, systemImage: s.symbol).tag(s)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200)
        } detail: {
            content(for: section ?? landingSection)
        }
        .overlay(alignment: .bottom) { NowPlayingBar() }
        .onAppear { if section == nil { section = landingSection } }
        #else
        TabView(selection: $section) {
            ForEach(AppSection.allCases) { s in
                NavigationStack { content(for: s) }
                    .tabItem { Label(s.rawValue, systemImage: s.symbol) }
                    .tag(Optional(s))
            }
        }
        .onAppear { if section == nil { section = landingSection } }
        .fullScreenCover(isPresented: Binding(get: { playback.item != nil }, set: { if !$0 { playback.stop() } })) {
            PlayerView()
        }
        #endif
    }

    @ViewBuilder
    private func content(for section: AppSection) -> some View {
        switch section {
        case .live: ChannelsView(kinds: [.live, .radio], title: "Live TV")
        case .guide: GuideView()
        case .onDemand: ChannelsView(kinds: [.movie, .episode], title: "On Demand")
        case .library: LocalLibraryView()
        case .sources: SourcesView()
        }
    }
}

#if os(macOS)
/// Persistent player strip on macOS (the player itself opens in a window).
struct NowPlayingBar: View {
    @EnvironmentObject private var playback: PlaybackController
    @State private var showPlayer = false

    var body: some View {
        if let item = playback.item {
            HStack {
                Text(item.title).lineLimit(1)
                Spacer()
                Text(statusText).foregroundStyle(.secondary)
                Button("Show") { showPlayer = true }
                Button("Stop") { playback.stop() }
            }
            .padding(10)
            .background(.regularMaterial)
            .sheet(isPresented: $showPlayer) { PlayerView().frame(minWidth: 800, minHeight: 500) }
        }
    }

    private var statusText: String {
        switch playback.state {
        case .idle: return ""
        case .connecting: return "Connecting"
        case .buffering: return "Buffering"
        case .playing: return "Playing"
        case .paused: return "Paused"
        case .ended: return "Ended"
        case .failed(let m): return "Failed: \(m)"
        }
    }
}
#endif
