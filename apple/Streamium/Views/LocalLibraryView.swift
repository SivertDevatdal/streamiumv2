import SwiftUI
import StreamiumKit
import UniformTypeIdentifiers

/// Plays the user's own files (Files app, iCloud Drive, external drives).
struct LocalLibraryView: View {
    @EnvironmentObject private var playback: PlaybackController
    @State private var showPicker = false
    @State private var recents: [URL] = []

    var body: some View {
        List {
            Section {
                Button {
                    showPicker = true
                } label: {
                    Label("Open a video or audio file…", systemImage: "folder.badge.plus")
                }
            }
            if !recents.isEmpty {
                Section("Recent") {
                    ForEach(recents, id: \.self) { url in
                        Button(url.lastPathComponent) { open(url) }
                    }
                }
            }
        }
        .navigationTitle("Library")
        .fileImporter(isPresented: $showPicker, allowedContentTypes: [.movie, .audio, .mpeg2TransportStream, .item]) { result in
            if case .success(let url) = result { open(url) }
        }
    }

    private func open(_ url: URL) {
        let accessed = url.startAccessingSecurityScopedResource()
        defer { if accessed { url.stopAccessingSecurityScopedResource() } }
        recents.removeAll { $0 == url }
        recents.insert(url, at: 0)
        let format = inferStreamFormat(url: url.absoluteString)
        playback.play(PlaybackItem(url: url, title: url.lastPathComponent, format: format, isLive: false))
    }
}
