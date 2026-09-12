import SwiftUI
import StreamiumKit
import StreamiumCore

/// Now/next guide. The full grid view comes in phase 1; this list already
/// answers "what is on" for every channel with guide data.
struct GuideView: View {
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var playback: PlaybackController
    @State private var query = ""

    private var channels: [FfiChannel] {
        let base = query.isEmpty ? library.channels : library.search(query)
        return base.filter { $0.kind == .live }
    }

    var body: some View {
        List(channels, id: \.id) { channel in
            let nn = library.nowNext(for: channel)
            VStack(alignment: .leading, spacing: 4) {
                Text(channel.name).font(.headline)
                if let now = nn?.now {
                    Label(now.title, systemImage: "play.circle").font(.subheadline)
                    if let next = nn?.next {
                        Text("Next: \(next.title) at \(time(next.start))").font(.caption).foregroundStyle(.secondary)
                    }
                } else {
                    Text("No guide data").font(.caption).foregroundStyle(.tertiary)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                if let item = PlaybackItem(channel: channel) { playback.play(item) }
            }
        }
        .searchable(text: $query)
        .navigationTitle("Guide")
        .overlay {
            if library.guideProgrammeCount == 0 {
                ContentUnavailableView("No guide loaded", systemImage: "list.bullet.rectangle",
                                       description: Text("Add a source with an XMLTV guide URL."))
            }
        }
    }

    private func time(_ unix: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(unix)).formatted(date: .omitted, time: .shortened)
    }
}
