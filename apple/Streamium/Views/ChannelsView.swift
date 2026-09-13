import SwiftUI
import StreamiumKit
import StreamiumCore

struct ChannelsView: View {
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var playback: PlaybackController
    let kinds: Set<FfiMediaKind>
    let title: String
    @State private var query = ""
    @State private var group: String? = nil
    @State private var favouritesOnly = false
    @State private var showAdd = false

    private var visible: [FfiChannel] {
        var list: [FfiChannel]
        if !query.trimmingCharacters(in: .whitespaces).isEmpty {
            list = library.search(query)
        } else if favouritesOnly {
            list = library.favourites
        } else {
            list = library.channels(inGroup: group)
        }
        return list.filter { kinds.contains($0.kind) }
    }

    private var groups: [String] {
        Set(library.channels.filter { kinds.contains($0.kind) }.compactMap(\.group)).sorted()
    }

    var body: some View {
        List(visible, id: \.id) { channel in
            ChannelRow(channel: channel)
                .contentShape(Rectangle())
                .onTapGesture { play(channel) }
                .swipeActions {
                    Button {
                        library.toggleFavourite(channel)
                    } label: {
                        Label("Favourite", systemImage: library.favouriteIDs.contains(channel.id) ? "star.slash" : "star")
                    }
                    .tint(.yellow)
                }
                .contextMenu {
                    Button(library.favouriteIDs.contains(channel.id) ? "Remove Favourite" : "Add Favourite") {
                        library.toggleFavourite(channel)
                    }
                }
        }
        .searchable(text: $query, prompt: "Search channels")
        .navigationTitle(title)
        .overlay {
            if library.channels.isEmpty {
                ContentUnavailableView {
                    Label("No channels yet", systemImage: "tv")
                } description: {
                    Text("Streamium ships no channels. Add the provider or playlist you use and they appear here.")
                } actions: {
                    Button("Add a Source") { showAdd = true }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .sheet(isPresented: $showAdd) { AddSourceView() }
        .toolbar {
            ToolbarItem {
                Toggle(isOn: $favouritesOnly) { Label("Favourites", systemImage: "star") }
            }
            ToolbarItem {
                Menu {
                    Button("All groups") { group = nil }
                    ForEach(groups, id: \.self) { g in Button(g) { group = g } }
                } label: {
                    Label(group ?? "All groups", systemImage: "line.3.horizontal.decrease.circle")
                }
            }
        }
    }

    private func play(_ channel: FfiChannel) {
        guard let item = PlaybackItem(channel: channel) else { return }
        playback.play(item)
    }
}

struct ChannelRow: View {
    @EnvironmentObject private var library: Library
    let channel: FfiChannel

    var body: some View {
        HStack(spacing: 12) {
            AsyncImage(url: channel.logo.flatMap { URL(string: $0) }) { image in
                image.resizable().scaledToFit()
            } placeholder: {
                Image(systemName: channel.kind == .radio ? "radio" : "tv")
                    .foregroundStyle(.secondary)
            }
            .frame(width: 44, height: 32)

            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    if let n = channel.number { Text("\(n)").font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
                    Text(channel.name).lineLimit(1)
                    if library.favouriteIDs.contains(channel.id) {
                        Image(systemName: "star.fill").font(.caption2).foregroundStyle(.yellow)
                    }
                }
                if let nn = library.nowNext(for: channel), let now = nn.now {
                    HStack(spacing: 6) {
                        Text(now.title).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        ProgressView(value: progress(now)).frame(width: 60)
                    }
                } else if let g = channel.group {
                    Text(g).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                }
            }
            Spacer()
            if channel.catchupKind != nil {
                Image(systemName: "clock.arrow.circlepath").foregroundStyle(.secondary).help("Catch-up available")
            }
        }
        .padding(.vertical, 2)
    }

    private func progress(_ p: FfiProgramme) -> Double {
        let now = Date().timeIntervalSince1970
        let d = Double(p.stop - p.start)
        guard d > 0 else { return 0 }
        return min(1, max(0, (now - Double(p.start)) / d))
    }
}
